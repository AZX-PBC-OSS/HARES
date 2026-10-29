//! Python bindings for dwelling simulation.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, FixedOffset};
use hares_control::PriceSignal;
use hares_core::{
    ActorConfig, ActorRegistry, BatteryLutData, Dwelling, DwellingBlueprint,
    DwellingBlueprintBuilder, DwellingCheckpoint, DwellingConfig, PremiseZip,
    environment::SurfaceGeometry,
};
use hares_equipment::{
    BatteryConfig, BatteryLutType, EquipmentConfig, EquipmentRegistry, EvConfig,
    ProtocolBridgeConfig, PvConfig, config::ConfigValue,
};
use hares_types::panic_hook::{self, PanicHookGuard};
use hares_types::{BatteryChemistry, EvConnectionState, HaresError, SurfaceIrradiance};
use pyo3::buffer::PyBuffer;
use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict, PyList, PyType};
use rand::SeedableRng;
use std::slice;
use std::sync::MutexGuard;

// The exceptions register under the extension module's importable path, so
// an exception pickled in one process unpickles in another: the parent
// imports `__module__` and looks the class up there.
pyo3::create_exception!(
    hares._hares,
    HaresConfigError,
    pyo3::exceptions::PyValueError
);
pyo3::create_exception!(
    hares._hares,
    HaresEquipmentError,
    pyo3::exceptions::PyValueError
);
pyo3::create_exception!(
    hares._hares,
    HaresSimulationError,
    pyo3::exceptions::PyRuntimeError
);
pyo3::create_exception!(hares._hares, HaresHealthPolicyError, HaresSimulationError);
pyo3::create_exception!(hares._hares, HaresCallbackError, HaresSimulationError);
pyo3::create_exception!(
    hares._hares,
    FatalDwellingError,
    pyo3::exceptions::PyRuntimeError
);

/// Test-only: the number of full dwelling re-constructions
/// `reset_with_seed` ran (its one `DwellingBlueprint::build_with_seed`
/// call each). Lives in `hares-python` because `cfg(test)` is not set for
/// `hares-core` when `hares-python`'s tests compile it: the reset the
/// environments use (`load_state` then `reseed`) must leave the counter
/// alone, and the counter is shown to count by a `reset_with_seed` call
/// moving it.
#[cfg(test)]
pub(crate) static TEST_BUILD_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// The construction counter's probe exclusivity: the probes assert exact
/// deltas on the process-global `TEST_BUILD_COUNT` (which every test
/// construction bumps), so no sibling construction may run between a
/// probe's samples. nextest's process isolation gives each probe its own
/// process, where the lock is uncontended; in-process runners (the
/// coverage job's llvm-cov cargo test) execute the tests on shared
/// threads, so every test in this file's modules holds the lock for its
/// whole body and the samples read the counter alone.
#[cfg(test)]
pub(crate) static TEST_COUNT_EXCLUSIVITY: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only fault switch for the batch step-failure classification: the
/// message returned as a NON-fatal step failure by every
/// [`PyDwelling::step_core_string`] call while set. `None` faults nothing.
/// Lives in `hares-python` (the crate under test), never in `hares-core`'s
/// `step`: `cfg(test)` is set only for the crate under test.
#[cfg(test)]
pub(crate) static TEST_STEP_FAILURE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Test-only fault switch for the batch's panic classification: every
/// [`PyDwelling::step_core_string`] call panics while its lock is held
/// when set, so the mutex genuinely poisons and `batch_step_py`'s panic
/// branch runs against a poisoned lock. `false` panics nobody. Lives in
/// `hares-python` (the crate under test), never in `hares-core`'s `step`:
/// `cfg(test)` is set only for the crate under test.
#[cfg(test)]
pub(crate) static TEST_STEP_PANIC: AtomicBool = AtomicBool::new(false);

use crate::conversions::naive_chrono_to_py_datetime;
use crate::py_actor::PyActor;
use crate::py_actor::PyPriority;
use crate::py_config::PySimulationConfig;
use crate::py_control::PyControlSignal;
use crate::py_enums::PyLutType;
use crate::py_enums::{PyEvArchetypeId, PyVehicleId};
use crate::py_equipment::{
    PyBattery, PyEquipment, PyEquipmentDescriptor, PyEv, PyProtocolBridge, PyPv,
    extract_charging_lut, extract_ocv_table, extract_u_neg_table,
};
use crate::py_result::{PyRunHealth, PyRunResult};
use crate::py_telemetry::PyTelemetry;
use crate::utils::finite_f64_optional;
use crate::utils::validate_finite;

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
        if col.starts_with("s")
            && col.ends_with("_direct")
            && let Ok(sid) = col[1..col.len() - 7].parse::<u32>()
        {
            surface_ids.push(sid);
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

    let first_surface: Bound<'_, PyDict> = dict
        .get_item(surface_ids[0])?
        .ok_or_else(|| {
            PyValueError::new_err(format!(
                "missing required key: surface id {}",
                surface_ids[0]
            ))
        })?
        .extract()?;
    let direct_val = first_surface
        .get_item("direct")?
        .ok_or_else(|| PyValueError::new_err("missing required key: 'direct'"))?;
    let n_timesteps: usize = get_array_len(&direct_val)?;

    let mut result: Vec<Vec<SurfaceIrradiance>> = Vec::with_capacity(n_timesteps);

    for step_idx in 0..n_timesteps {
        let mut surfaces = Vec::with_capacity(surface_ids.len());
        for &sid in &surface_ids {
            let surface_dict: Bound<'_, PyDict> = dict
                .get_item(sid)?
                .ok_or_else(|| {
                    PyValueError::new_err(format!("missing required key: surface id {sid}"))
                })?
                .extract()?;

            let direct: f64 = extract_array_element(
                &surface_dict
                    .get_item("direct")?
                    .ok_or_else(|| PyValueError::new_err("missing required key: 'direct'"))?,
                step_idx,
            )?;
            let diffuse: f64 = extract_array_element(
                &surface_dict
                    .get_item("diffuse")?
                    .ok_or_else(|| PyValueError::new_err("missing required key: 'diffuse'"))?,
                step_idx,
            )?;
            let reflected: f64 = extract_array_element(
                &surface_dict
                    .get_item("reflected")?
                    .ok_or_else(|| PyValueError::new_err("missing required key: 'reflected'"))?,
                step_idx,
            )?;
            let aoi: f64 = extract_array_element(
                &surface_dict
                    .get_item("aoi")?
                    .ok_or_else(|| PyValueError::new_err("missing required key: 'aoi'"))?,
                step_idx,
            )?;

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

/// Render a [`PremiseZip`] as the Python dict documented on
/// `Dwelling.premise_zip`.
fn premise_zip_to_pydict<'py>(
    py: Python<'py>,
    premise: &PremiseZip,
) -> PyResult<Bound<'py, PyDict>> {
    let zip = &premise.zip;
    let out = PyDict::new(py);
    out.set_item("zp", zip.zp)?;
    out.set_item("ip", zip.ip)?;
    out.set_item("pp", zip.pp)?;
    out.set_item("zq", zip.zq)?;
    out.set_item("iq", zip.iq)?;
    out.set_item("pq", zip.pq)?;
    out.set_item("pf", zip.pf)?;
    out.set_item("v0", zip.v0)?;
    // The aggregate is over ZIP-governing equipment by construction; the
    // flag keeps the dict shape consistent with equipment_zip's.
    out.set_item("real_power_zip_applies", true)?;
    // The assembled governed response at the aggregation's bus voltage
    // (the moment's composition, each row's deep-sag form applied). None
    // when no governing equipment carried a positive weight.
    out.set_item("deep_sag_scale", premise.deep_sag_scale)?;
    let governing = PyList::empty(py);
    for (name, mean_power_w) in &premise.governing {
        let name: &str = name.as_str();
        let entry = PyDict::new(py);
        entry.set_item("name", name)?;
        entry.set_item("mean_power_w", *mean_power_w)?;
        governing.append(entry)?;
    }
    out.set_item("governing_equipment", governing)?;
    let constant_power = PyList::empty(py);
    for name in &premise.constant_power {
        let name: &str = name.as_str();
        constant_power.append(name)?;
    }
    out.set_item("constant_power_equipment", constant_power)?;
    Ok(out)
}

fn get_array_len(arr: &Bound<'_, PyAny>) -> PyResult<usize> {
    if let Ok(list) = arr.extract::<Bound<'_, PyList>>() {
        return Ok(list.len());
    }
    if arr.hasattr("shape")?
        && let Ok(shape) = arr.getattr("shape")
        && let Ok(tuple) = shape.extract::<Bound<'_, pyo3::types::PyTuple>>()
        && tuple.len() > 0
        && let Ok(Ok(dim)) = tuple.get_item(0).map(|v| v.extract::<usize>())
    {
        return Ok(dim);
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

            let surface_id: u32 = surface_dict
                .get_item("surface_id")?
                .ok_or_else(|| PyValueError::new_err("missing required key: 'surface_id'"))?
                .extract()?;
            let direct_w_m2: f64 = surface_dict
                .get_item("direct_w_m2")?
                .ok_or_else(|| PyValueError::new_err("missing required key: 'direct_w_m2'"))?
                .extract()?;
            let diffuse_w_m2: f64 = surface_dict
                .get_item("diffuse_w_m2")?
                .ok_or_else(|| PyValueError::new_err("missing required key: 'diffuse_w_m2'"))?
                .extract()?;
            let reflected_w_m2: f64 = surface_dict
                .get_item("reflected_w_m2")?
                .ok_or_else(|| PyValueError::new_err("missing required key: 'reflected_w_m2'"))?
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

fn battery_config_from_py(battery: &PyBattery) -> Result<EquipmentConfig, HaresError> {
    let cfg = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_j: battery.capacity_j,
        max_charge_w: battery.max_charge_w.unwrap_or(5000.0),
        max_discharge_w: battery.max_discharge_w.unwrap_or(5000.0),
        power_factor: battery.power_factor,
        inverter_capacity_va: battery.inverter_capacity_va,
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
        grid_forming: None,
        min_dwell_steps: 0,
        der_1547: battery.der_1547.clone(),
    };
    EquipmentConfig::from_typed(battery.name.clone(), "Battery".to_string(), cfg)
}

fn pv_config_from_py(pv: &PyPv) -> Result<EquipmentConfig, HaresError> {
    let soiling = pv
        .soiling
        .as_ref()
        .map(|s| hares_equipment::pv::soiling::SoilingConfig {
            cleaning_threshold_m: s.cleaning_threshold_mm / 1000.0,
            rain_accum_period_s: s.rain_accum_hours * 3600.0,
            soiling_loss_rate_per_s: s.loss_rate_per_day / 86_400.0,
            grace_period_s: s.grace_period_days * 86_400.0,
            max_soiling: s.max_loss,
            initial_soiling: s.initial_loss,
        });
    let cfg = PvConfig {
        equipment_id: None,
        zone_id: None,
        capacity_w: pv.capacity_w,
        tilt_deg: Some(pv.tilt),
        azimuth_deg: Some(pv.azimuth),
        attached_boundary_id: None,
        module_type: None,
        noct_c: None,
        array_type: None,
        system_losses_fraction: None,
        inverter_efficiency: None,
        inverter_capacity_w: None,
        power_factor: None,
        surface_resolution_deg: None,
        sam_lut_path: pv.sam_lut_path.clone(),
        soiling,
        arrays: None,
        der_1547: pv.der_1547.clone(),
    };
    EquipmentConfig::from_typed(pv.name.clone(), "PV".to_string(), cfg)
}

fn ev_config_from_py(ev: &PyEv) -> Result<EquipmentConfig, HaresError> {
    // The builder's defaults are datasheet values written in SI: 75 kWh
    // is 2.7e8 J and 7.2 or 11.5 kW is 7200 or 11500 W, converted once
    // with uom (the threshold 56.875 kWh is the same test in J).
    let capacity_j = ev.capacity_j.unwrap_or(270000000.0);
    let max_charging_power_w = ev.max_charging_power_w.unwrap_or_else(|| {
        if capacity_j
            < uom::si::f64::Energy::new::<uom::si::energy::kilowatt_hour>(56.875)
                .get::<uom::si::energy::joule>()
        {
            7200.0
        } else {
            11500.0
        }
    });
    let cfg = EvConfig {
        equipment_id: None,
        capacity_j,
        charging_level: None,
        max_charging_power_w,
        charging_efficiency: None,
        l1_current_a: None,
        l1_voltage_v: None,
        soc_max: None,
        initial_soc: ev.initial_soc,
        battery_temp_c: ev.battery_temp_c,
        min_charge_temp_c: ev.min_charge_temp_c,
        full_power_temp_c: ev.full_power_temp_c,
        heater_power_w: ev.heater_power_w,
        heater_threshold_c: ev.heater_threshold_c,
        thermal_mass_j_per_k: ev.thermal_mass_j_per_k,
        ua_w_per_k: ev.ua_w_per_k,
        n_series: ev.n_series,
        n_parallel: ev.n_parallel,
        cell_resistance_ohm: ev.cell_resistance_ohm,
        v2l_enabled: None,
        v2l_soc_reserve: None,
        v2l_max_discharge_w: None,
        v2g_enabled: None,
        v2g_soc_reserve: None,
        v2g_max_discharge_w: None,
        chemistry: None,
        fuel_economy_j_per_m: None,
        ready_soc: None,
        charging_strategy: None,
        plug_in_policy: None,
        power_limit_w: None,
        initial_connection_state: ev
            .initial_connection_state
            .map(|state| EvConnectionState::from(state).to_string()),
        power_factor: ev.power_factor,
        charger_capacity_va: ev.charger_capacity_va,
        cc_cv_transition_soc: None,
        charging_priority: None,
        discharge_respects_deadline: true,
        der_1547: ev.der_1547.clone(),
    };
    EquipmentConfig::from_typed(ev.name.clone(), "EV".to_string(), cfg)
}

fn protocol_bridge_config_from_py(
    bridge: &PyProtocolBridge,
) -> Result<EquipmentConfig, HaresError> {
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

/// The fatal failure messages' leading text. It is part of the message's
/// own wording, for the human or log reader of the message, and no
/// classification reads it: fatality crosses the GIL-free boundary as
/// [`StepFailure`]'s `fatal` field, set by the paths that know the
/// dwelling's state.
const FATAL_DWELLING_PREFIX: &str = "[FATAL_DWELLING] ";

/// A step failure crossing the GIL-free batch boundary: the message and
/// whether the dwelling is permanently failed. `fatal` is carried as
/// data, set only by the paths that know the dwelling's state (the
/// poisoned flag, a poisoned lock, a panic that poisoned the lock), so
/// the receiving thread classifies the failure without reading message
/// content: a transient error's message is the error's own text, never
/// a classification.
#[derive(Debug, Clone)]
pub(crate) struct StepFailure {
    pub(crate) msg: String,
    pub(crate) fatal: bool,
}

/// The per-zone temperature keys a step dict carries, in the step's zone
/// order: "Temperature - Indoor (C)" for zone 0, "Temperature - Zone_N (C)"
/// beyond it.
pub(crate) fn zone_keys_for_step(step: &hares_core::StepResult) -> Vec<String> {
    step.zone_temperatures_c
        .iter()
        .map(|(zone_id, _)| {
            if zone_id.0 == 0 {
                "Temperature - Indoor (C)".to_string()
            } else {
                format!("Temperature - Zone_{} (C)", zone_id.0)
            }
        })
        .collect()
}

/// The dict `PyDwelling.step()` returns, built from one step result and the
/// zone keys [`zone_keys_for_step`] names: the one conversion both
/// `PyDwelling.step()` and every `batch_step` row go through, so both paths
/// hand Python the same step dict.
pub(crate) fn step_result_to_py_dict(
    py: Python<'_>,
    step: &hares_core::StepResult,
    zone_keys: &[String],
) -> PyResult<Py<PyAny>> {
    let out = PyDict::new(py);
    out.set_item(
        "timestamp",
        naive_chrono_to_py_datetime(py, step.timestamp)?,
    )?;
    out.set_item("net_electric_power_w", step.net_electric_power_w)?;
    out.set_item("hvac_heating_w", step.hvac_heating_w)?;
    out.set_item("hvac_cooling_w", step.hvac_cooling_w)?;
    out.set_item("gas_power_w", step.gas_power_w)?;
    for ((_zone_id, temp_c), key) in step.zone_temperatures_c.iter().zip(zone_keys.iter()) {
        out.set_item(key, *temp_c)?;
    }
    Ok(out.unbind().into())
}

/// The GIL state `construct_detached` observed inside the construction it
/// ran, recorded for the `construction_runs_detached` test: 0 is released,
/// 1 is held, -1 is no construction yet. Test-only, so no production call
/// pays for the store.
#[cfg(test)]
pub(crate) static CONSTRUCT_GIL_STATE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(-1);

/// The one detached construction the Python-facing dwelling constructors
/// share: the expensive parse+resolve runs with the GIL released, so
/// parallel callers construct concurrently. Argument extraction stays
/// attached, before the call, and the closure returns plain Rust values
/// (`HaresError` crosses as data; the `PyErr` conversion needs the GIL and
/// happens after). The audited construction path touches no Python-owned
/// state while released: hares-core and hares-io carry no pyo3 dependency,
/// and the batch callback attaches only to a built dwelling and
/// re-acquires the GIL itself at delivery.
pub(crate) fn construct_detached<T: Send>(py: Python<'_>, f: impl FnOnce() -> T + Send) -> T {
    py.detach(|| {
        #[cfg(test)]
        // Safe: the check only reads the calling thread's GIL state.
        CONSTRUCT_GIL_STATE.store(unsafe { pyo3::ffi::PyGILState_Check() }, Ordering::SeqCst);
        f()
    })
}

#[pyclass(name = "Dwelling")]
pub struct PyDwelling {
    pub(crate) dwelling: Mutex<Dwelling>,
    pub(crate) poisoned: AtomicBool,
    pub(crate) mutex_poison_events: std::sync::atomic::AtomicU64,
    pub(crate) config: DwellingConfig,
    /// The blueprint the dwelling was built from: the shared parses and
    /// the state every `reset_with_seed` rebuild clones.
    pub(crate) blueprint: Arc<DwellingBlueprint>,
    initialized: bool,
    registry: ActorRegistry,
    equipment_registry: EquipmentRegistry,
    /// The per-zone temperature keys a step dict carries, cached at the
    /// first dict build (the zone structure never changes under a built
    /// dwelling): the single-step path and the batch path's dict builds
    /// share the cache, so a step allocates no key strings after the
    /// first. The mutex is the batch path's handle: it builds dicts from
    /// a shared borrow of the dwelling on the parent thread.
    pub(crate) zone_keys: Mutex<Option<Vec<String>>>,
    /// The single-step loop's reused result: the step writes it in place
    /// (the `step_into` form), so a Python loop calling `step()` per
    /// timestep pays the zone-temperature allocation once, at the first
    /// step, and the loop's steady state allocates nothing for the
    /// result. The buffer is write-only within a step and re-derived
    /// before every read (the `step_into` contract), so no mutation
    /// point can leak a stale result.
    step_scratch: hares_core::StepResult,
    gas_tariff: Option<crate::py_tariff::PyGasTariff>,
    /// The attached batch callbacks' cause slot: one slot the dwelling
    /// owns, shared by every attached callback's adapter. The adapter
    /// stores the callable's exception there and the run-error mapping
    /// chains it as `__cause__`. Cleared when the dwelling is rebuilt.
    callback_cause: crate::py_callback::CallbackCauseSlot,
}

#[pymethods]
impl PyDwelling {
    #[classmethod]
    #[pyo3(signature = (hpxml, schedule, weather, config, *, bldg_id = 0, defaults = None, overrides = None, **unexpected))]
    // PyO3 #[classmethod] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: &Bound<'_, PyAny>,
        schedule: Option<&Bound<'_, PyAny>>,
        weather: &Bound<'_, PyAny>,
        config: crate::py_config::PySimulationConfig,
        bldg_id: i64,
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
                hpxml: crate::py_inputs::hpxml_arg(hpxml)?,
                schedule: crate::py_blueprint::schedule_arg_maybe(schedule)?,
                weather: crate::py_inputs::weather_arg(weather)?,
                defaults: match defaults {
                    Some(path) => crate::py_inputs::defaults_arg(path)?,
                    None => {
                        crate::py_inputs::DefaultsArg::Source(hares_io::DefaultsSource::directory(
                            crate::py_inputs::packaged_defaults_dir(_cls.py())?,
                        ))
                    }
                },
                config,
                bldg_id,
                overrides,
                unexpected,
            })?;
        let settings = config.settings.clone();
        let (dwelling, blueprint) = construct_detached(_cls.py(), move || {
            let inputs = crate::py_blueprint::builder_inputs(&inputs)?;
            let builder = DwellingBlueprintBuilder::new(inputs, settings)?;
            builder
                .freeze()
                .and_then(|blueprint| blueprint.build().map(|dwelling| (dwelling, blueprint)))
        })
        .map_err(to_py_err)?;
        Ok(Self {
            dwelling: Mutex::new(dwelling),
            poisoned: AtomicBool::new(false),
            mutex_poison_events: std::sync::atomic::AtomicU64::new(0),
            config: config.clone(),
            blueprint,
            initialized: false,
            registry: ActorRegistry::new(),
            equipment_registry: EquipmentRegistry::new(),
            zone_keys: Mutex::new(None),
            step_scratch: hares_core::StepResult::default(),
            gas_tariff: None,
            callback_cause: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// The blueprint the current dwelling was built from: the shared parses
    /// (one per unique input) and the state every build clones. A
    /// `reset_with_seed` rebuild reports the blueprint it re-froze (a
    /// generated diversified schedule re-derives under the new seed), so
    /// this accessor follows the dwelling the object currently carries.
    pub fn blueprint(&self) -> crate::py_blueprint::PyDwellingBlueprintRef {
        crate::py_blueprint::PyDwellingBlueprintRef {
            inner: Arc::clone(&self.blueprint),
        }
    }

    fn config(&self) -> PySimulationConfig {
        // Read through the dwelling's config, so a rebuilt one (a new seed)
        // is the config the object reports.
        PySimulationConfig(self.config.settings.sim_config.clone())
    }

    pub fn initialize(&mut self) -> PyResult<()> {
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

    /// Per-phase wall-clock split of the work done so far, behind the `profiling`
    /// feature (`maturin develop --release --features profiling`). A flat dict of
    /// seconds plus the memory high-water mark, the hot-path allocation
    /// counters (each `None` when no measurement is available: the extension
    /// does not install the counting allocator), and `per_actor`: the run's
    /// per-actor totals as `{"name", "total_s", "calls"}` dicts, one per
    /// registered actor in registration order. Goes through the crate's own
    /// acquire discipline: a fatal-error dwelling refuses, and lock poison surfaces as
    /// the same error type every other method raises. Without the feature the
    /// method raises `NotImplementedError` instead of returning zeros that
    /// would read as a measurement.
    pub fn profiling_summary(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        #[cfg(not(feature = "profiling"))]
        {
            let _ = py;
            Err(pyo3::exceptions::PyNotImplementedError::new_err(
                "PyDwelling.profiling_summary() requires building the extension with the 'profiling' feature: maturin develop --release --features profiling",
            ))
        }
        #[cfg(feature = "profiling")]
        {
            let d = pyo3::types::PyDict::new(py);
            // acquire discipline: a fatal-error dwelling refuses, and lock poison
            // surfaces as the same error type every other method raises.
            let p = self.acquire()?.profiling_summary();
            d.set_item("environment_s", p.environment.as_secs_f64())?;
            d.set_item("control_s", p.control.as_secs_f64())?;
            d.set_item("ideal_capacity_s", p.ideal_capacity.as_secs_f64())?;
            d.set_item("actors_s", p.actors.as_secs_f64())?;
            d.set_item("dispatch_s", p.dispatch.as_secs_f64())?;
            d.set_item("equipment_s", p.equipment.as_secs_f64())?;
            d.set_item("envelope_s", p.envelope.as_secs_f64())?;
            d.set_item("invariants_s", p.invariants.as_secs_f64())?;
            d.set_item("state_snapshot_s", p.state_snapshot.as_secs_f64())?;
            d.set_item("output_s", p.output.as_secs_f64())?;
            d.set_item("accounting_s", p.accounting.as_secs_f64())?;
            d.set_item("step_total_s", p.step_total.as_secs_f64())?;
            d.set_item("finish_s", p.finish.map(|d| d.as_secs_f64()))?;
            d.set_item("memory_high_water_kb", p.memory_high_water_kb)?;
            d.set_item("hot_path_allocations", p.hot_path_allocations)?;
            d.set_item("hot_path_alloc_violations", p.hot_path_alloc_violations)?;
            let per_actor_entries: Vec<pyo3::Bound<'_, pyo3::types::PyDict>> = p
                .per_actor
                .iter()
                .map(|timing| {
                    let entry = pyo3::types::PyDict::new(py);
                    entry.set_item("name", timing.name.as_ref())?;
                    entry.set_item("total_s", timing.total.as_secs_f64())?;
                    entry.set_item("calls", timing.calls)?;
                    Ok(entry)
                })
                .collect::<PyResult<_>>()?;
            let per_actor = pyo3::types::PyList::new(py, per_actor_entries)?;
            d.set_item("per_actor", per_actor)?;
            Ok(d.into())
        }
    }

    /// Runs the full configured horizon and returns the finished run's
    /// result: the one finalisation a stepped run takes through
    /// `finish()` is the same path this takes at the horizon's end.
    pub fn simulate(&self, py: Python<'_>) -> PyResult<PyRunResult> {
        let _guard = PanicHookGuard::new();
        #[cfg(debug_assertions)]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before simulation"
            );
        }
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "PyDwelling.simulate() called before initialize(). Call dwelling.initialize() first.",
            ));
        }
        let sim_result: Result<Result<std::sync::Arc<hares_core::RunResult>, HaresError>, _> = py
            .detach(|| {
                std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let mut dwelling = self.acquire_hares()?;
                    dwelling.simulate()
                }))
            });
        let result = match sim_result {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                if self.poisoned.load(Ordering::Acquire) {
                    return Err(FatalDwellingError::new_err(e.to_string()));
                }
                return Err(self.map_run_error(e));
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

        #[cfg(debug_assertions)]
        {
            assert!(
                !self.poisoned.load(Ordering::Acquire) || self.dwelling.is_poisoned(),
                "invariant violation: poisoned flag is true but dwelling mutex is not poisoned"
            );
        }

        let catalogue = { self.acquire()?.catalogue() };
        Ok(PyRunResult {
            inner: result,
            catalogue,
        })
    }

    /// Finalises the run in place: the billing close, the post-hoc
    /// diagnostics, the recorders' flush, close and publish (the final
    /// partial batch included), the metrics, the health, the warning
    /// record and the provenance, and returns the finished run's result.
    /// Idempotent: a call after the first returns the first call's result.
    /// A run that has stepped nothing cannot finish.
    pub fn finish(&self, py: Python<'_>) -> PyResult<PyRunResult> {
        let _guard = PanicHookGuard::new();
        #[cfg(debug_assertions)]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before finish"
            );
        }
        let finish_outcome = py.detach(|| {
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mut dwelling = self.acquire_hares()?;
                dwelling.finish()
            }))
        });
        let finish_result = match finish_outcome {
            Ok(result) => result,
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
        match finish_result {
            Ok(result) => {
                let catalogue = { self.acquire()?.catalogue() };
                Ok(PyRunResult {
                    inner: result,
                    catalogue,
                })
            }
            Err(e) => {
                if self.poisoned.load(Ordering::Acquire) {
                    return Err(FatalDwellingError::new_err(e.to_string()));
                }
                Err(self.map_run_error(e))
            }
        }
    }

    /// The run's live health totals: readable at any step, needing no
    /// report.
    pub fn health(&self) -> PyResult<PyRunHealth> {
        if crate::py_callback::delivery_in_progress() {
            return Err(reentry_refusal());
        }
        let dwelling = self.acquire()?;
        Ok(PyRunHealth {
            inner: dwelling.health().clone(),
        })
    }

    pub fn step(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let _guard = PanicHookGuard::new();
        #[cfg(debug_assertions)]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before step"
            );
        }
        if !self.initialized {
            return Err(PyRuntimeError::new_err(
                "PyDwelling.step() called before initialize(). Call dwelling.initialize() first.",
            ));
        }
        // The loop scratch is taken out for the step and returned after
        // it, so the GIL-free section touches no Python state. The step
        // writes the reused result in place; on any error the dict is
        // never built and the buffer is unread, and every field is
        // overwritten on the success path, so a mutation point (a
        // control signal, an equipment override, a checkpoint restore, a
        // reseed or reset) cannot leak a stale result into a later dict.
        let mut scratch = std::mem::take(&mut self.step_scratch);
        let stepped: Result<Result<(), HaresError>, _> = py.detach(|| {
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mut dwelling = self.acquire_hares()?;
                dwelling.step_into(&mut scratch)
            }))
        });
        self.step_scratch = scratch;
        match stepped {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if self.poisoned.load(Ordering::Acquire) {
                    return Err(FatalDwellingError::new_err(e.to_string()));
                }
                return Err(self.map_run_error(e));
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

        #[cfg(debug_assertions)]
        {
            assert!(
                !self.poisoned.load(Ordering::Acquire) || self.dwelling.is_poisoned(),
                "invariant violation: poisoned flag is true but dwelling mutex is not poisoned"
            );
        }

        let mut cache = self
            .zone_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let zone_keys = cache.get_or_insert_with(|| zone_keys_for_step(&self.step_scratch));
        step_result_to_py_dict(py, &self.step_scratch, zone_keys)
    }

    #[pyo3(signature = (name, signal, priority=None))]
    pub fn apply_control(
        &self,
        name: String,
        signal: &PyControlSignal,
        priority: Option<PyPriority>,
    ) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        let tier = priority.map(|p| p.into_priority_tier());
        dwelling
            .apply_control_validated(&name, signal.signal.clone(), tier)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Adds a Python actor to the dwelling's decision-making loop.
    pub fn add_actor(&self, py: Python<'_>, actor: Py<PyActor>) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        let wrapper = crate::py_actor::PyActorWrapper::new(py, actor)?;
        dwelling.add_actor(Box::new(wrapper)).map_err(to_py_err)?;
        Ok(())
    }

    /// Creates and adds an actor from the registry using configuration.
    ///
    /// # Arguments
    ///
    /// * `actor_type` - Registered actor type name (e.g., "IdealThermostat", "Occupant", "DrCompliance")
    /// * `name` - Actor instance name
    /// * `params` - Configuration parameters as a dictionary
    #[pyo3(signature = (actor_type, name, params=None))]
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

        // Every refusal here is of the caller's input: an unknown type, a
        // bad parameter, a duplicate name, or an event the dwelling's
        // equipment cannot serve.
        let mut dwelling = self.acquire()?;
        // The dwelling identity the actor's key material keys on (the EV
        // driver's driving sources): the config-stable building id.
        config.dwelling_id = dwelling.bldg_id;
        dwelling
            .add_actor_by_name(&self.registry, config)
            .map_err(|e| HaresConfigError::new_err(e.to_string()))?;
        Ok(())
    }

    pub fn set_price_signal(&self, signal: &Bound<'_, PyDict>) -> PyResult<()> {
        // Loud on non-finite prices at the boundary (the shared
        // validators' section rationale, `utils.rs`): a NaN price's
        // comparisons are always false, so the price actors'
        // conditionals (the EvDriver's price logic) silently stop
        // firing in release builds.
        let price = PriceSignal {
            electricity_price: finite_f64_optional(signal, "electricity_price")?,
            export_price: finite_f64_optional(signal, "export_price")?,
            ghg_intensity: finite_f64_optional(signal, "ghg_intensity")?,
        };
        let mut dwelling = self.acquire()?;
        dwelling.set_price_signal(price);
        Ok(())
    }

    pub fn set_grid_voltage(&self, voltage_pu: f64) -> PyResult<()> {
        // Loud on non-finite per-unit voltage at the boundary (the
        // shared validators' section rationale, `utils.rs`): a NaN
        // voltage silently lands in the grid state and stops every
        // per-unit comparison downstream. The engine's setter validates
        // the full contract (exactly 0.0 or a finite value in
        // [0.5, 1.5] pu); finiteness is re-checked here so the Python
        // caller sees the boundary's error class for NaN before the
        // dwelling is acquired.
        validate_finite(voltage_pu, "voltage_pu")?;
        let mut dwelling = self.acquire()?;
        dwelling.set_grid_voltage(voltage_pu).map_err(to_py_err)?;
        Ok(())
    }

    pub fn set_frequency(&self, frequency_hz: f64) -> PyResult<()> {
        // Loud on non-finite frequency at the boundary (the shared
        // validators' section rationale, `utils.rs`). The engine's
        // setter validates the full contract (a finite value in
        // [40, 80] Hz); finiteness is re-checked here so the Python
        // caller sees the boundary's error class for NaN before the
        // dwelling is acquired.
        validate_finite(frequency_hz, "frequency_hz")?;
        let mut dwelling = self.acquire()?;
        dwelling.set_frequency(frequency_hz).map_err(to_py_err)?;
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
        if crate::py_callback::delivery_in_progress() {
            return Err(reentry_refusal());
        }
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
        Ok(PyTelemetry::new(dwelling.telemetry().map_err(to_py_err)?))
    }

    /// The dwelling's output variable catalogue: every variable the built
    /// model can produce, declared once, typed, with its source. Readable
    /// from build on, before the first step; rebuilt on every equipment or
    /// actor roster change.
    pub fn catalogue(&self) -> PyResult<crate::py_catalogue::PyCatalogue> {
        let dwelling = self.acquire()?;
        Ok(crate::py_catalogue::PyCatalogue {
            inner: dwelling.catalogue(),
        })
    }

    /// The resolved-parameter read-back: the parameters the physics
    /// actually uses, computed at assembly, after defaults, autosizing and
    /// overrides and before warm-up. A dict of SI floats and strings: per
    /// equipment its resolved parameters and the active setpoint sources
    /// (their provenance labels), the boundaries' effective R, C and
    /// layers, the resolved windows, the infiltration inputs and the site.
    pub fn resolved_model(&self) -> PyResult<Py<PyAny>> {
        let dwelling = self.acquire()?;
        Python::attach(|py| resolved_model_to_py(py, dwelling.resolved_model()))
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

    /// Each equipment with its core output for the latest step. After a
    /// tolerated step failure that equipment's flows read zero (it delivered
    /// nothing) and its state is the committed one.
    pub fn equipment(&self) -> PyResult<Vec<PyEquipment>> {
        let dwelling = self.acquire()?;
        Ok(dwelling
            .equipment()
            .iter()
            .enumerate()
            .map(|(idx, eq)| {
                PyEquipment::new(
                    eq.descriptor().clone(),
                    dwelling.reported_core_output(idx),
                    eq.telemetry().clone(),
                    eq.resolved_zip(),
                )
            })
            .collect())
    }

    /// The ``(low, high)`` range in °C a ``ThermalSetpoint`` deadband sent
    /// to the named equipment is held to (its thermostat class), or
    /// ``None`` when it takes no band. Raises ``ValueError`` for an unknown
    /// equipment name.
    pub fn thermostat_band_range(&self, name: &str) -> PyResult<Option<(f64, f64)>> {
        self.band_range_of(name).map_err(PyValueError::new_err)
    }

    /// The setpoints the named equipment's thermostat serves, as
    /// ``["Heating"]``, ``["Cooling"]`` or ``["Heating", "Cooling"]``, or
    /// ``None`` when it has no setpoint a pre-conditioning event can move.
    /// A pre-conditioning event on a unit serving both must name its
    /// direction (``SetpointAdjust:<delta>:PreHeat`` or ``:PreCool``).
    /// Raises ``ValueError`` for an unknown equipment name.
    pub fn thermostat_axes(&self, name: &str) -> PyResult<Option<Vec<&'static str>>> {
        use hares_types::{ThermostatAxes, ThermostatAxis};
        let axis_name = |axis: ThermostatAxis| match axis {
            ThermostatAxis::Heating => "Heating",
            ThermostatAxis::Cooling => "Cooling",
        };
        let dwelling = self.acquire()?;
        let eq = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| PyValueError::new_err(format!("equipment '{name}' not found")))?;
        Ok(eq.thermostat_axes().map(|axes| match axes {
            ThermostatAxes::One(axis) => vec![axis_name(axis)],
            ThermostatAxes::Both => vec![
                axis_name(ThermostatAxis::Heating),
                axis_name(ThermostatAxis::Cooling),
            ],
        }))
    }

    /// The primary resolved ZIP/power-factor model for the named equipment,
    /// as a dict with keys ``zp``/``ip``/``pp``/``zq``/``iq``/``pq``/``pf``/
    /// ``v0``/``real_power_zip_applies``, or ``None`` when the equipment has
    /// no electrical ZIP concept (e.g. generator, protocol bridge,
    /// indirect tank).
    ///
    /// ``real_power_zip_applies`` declares whether the real-power
    /// coefficients govern the equipment's real power: ``True`` for
    /// scheduled/event loads (whose scheduled draw is scaled by the
    /// real-power polynomial at the bus voltage) and for typed physics
    /// equipment (HVAC, water heaters, ventilation: the physics-computed
    /// draw is scaled by the class row through the recorded deep-sag form,
    /// so the published coefficients are the row's own and are real),
    /// ``False`` for DER (real power is controller-driven and never
    /// ZIP-scaled; only the reactive coefficients apply). Anything
    /// computing voltage sensitivity from the published real coefficients
    /// must check this flag first; for the whole-dwelling answer use
    /// [`Dwelling::premise_zip`].
    ///
    /// For DER with live var control (battery, EV, PV) ``pf`` is the current
    /// effective power factor (config baseline, later mutated by a
    /// ``PowerFactorSetpoint``). Raises ``ValueError`` for an unknown
    /// equipment name. Pure inspection — never influences the simulation.
    pub fn equipment_zip<'py>(
        &self,
        py: Python<'py>,
        name: &str,
    ) -> PyResult<Option<Bound<'py, PyDict>>> {
        let dwelling = self.acquire()?;
        let eq = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| PyValueError::new_err(format!("equipment '{}' not found", name)))?;
        eq.resolved_zip()
            .map(|zip| crate::py_equipment::zip_to_pydict(py, &zip))
            .transpose()
    }

    /// The dwelling's aggregate real-power voltage response: the
    /// power-weighted mean ZIP over the equipment whose real-power
    /// coefficients genuinely govern their real power (scheduled and event
    /// loads, and the typed physics equipment whose rows scale their
    /// physics-computed draws). The weights are the moment's raw draws when
    /// the dwelling has stepped (each equipment's last electric draw,
    /// un-scaled through its assembled deep-sag response at the bus voltage
    /// (the equipment's own nominal draw when it reports one: a
    /// multi-component unit, or a state its row does not scale such as a
    /// commanded power override; the resolved row's un-scale otherwise, the
    /// moment's composition)), else the expected mean draw over the loaded
    /// schedule data.
    ///
    /// Returns a dict with keys ``zp``/``ip``/``pp``/``zq``/``iq``/``pq``/
    /// ``pf``/``v0`` (the aggregate coefficients; the real side sums to 1,
    /// the reactive side is a first-order aggregate),
    /// ``real_power_zip_applies`` (always ``True`` — the aggregate describes
    /// the governing mix), ``deep_sag_scale`` (the assembled governed
    /// response at the current bus voltage: the weighting-draw-weighted mean
    /// of each governing equipment's own deep-sag response: the
    /// components' actual evaluated draws over the components' nominal
    /// draws for a stepped multi-component unit, the resolved row's
    /// recorded deep-sag form for a row-scaled step, the flat identity for
    /// a step the row does not scale (a commanded power override): the
    /// number the stepping engine answers for the moment's composition;
    /// ``None`` when no governing equipment carried a positive weight),
    /// ``governing_equipment`` (list of
    /// ``{"name": str, "mean_power_w": float | None}`` -- equipment
    /// included in the aggregate with its weighting draw in watts;
    /// ``None`` marks equipment whose weight is not computable, which
    /// contributes no weight), and ``constant_power_equipment`` (list of
    /// names whose real power is DER-controller-driven and therefore
    /// voltage-invariant by construction (battery, EV, PV).
    ///
    /// At the de-energized boundary (the admitted 0.0 pu outage value) the
    /// aggregate reports the de-energized state: a load that stepped and
    /// drew zero carries that zero weight, no expected mean stands in for
    /// any load, and the aggregate is ``None``: the engine draws zero from
    /// every load on a de-energized bus. On an energized bus whose voltage
    /// a load's row cannot draw at (a non-positive evaluation, the
    /// near-dead edge) that load's expectation carries no weight either,
    /// and ``deep_sag_scale`` is always a positive finite multiplier when
    /// reported.
    ///
    /// ``None`` when no ZIP-governing equipment has a computable positive
    /// weight; never a silent constant-power answer. Pure
    /// inspection — never influences the simulation.
    pub fn premise_zip<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        let dwelling = self.acquire()?;
        let Some(premise) = dwelling.premise_zip() else {
            return Ok(None);
        };
        Ok(Some(premise_zip_to_pydict(py, &premise)?))
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
        let config = battery_config_from_py(battery)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

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

        dwelling.add_equipment(eq).map_err(to_py_err)?;
        Ok(())
    }

    pub fn add_pv(&mut self, pv: &PyPv) -> PyResult<()> {
        let config = pv_config_from_py(pv)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

        let mut eq = self
            .equipment_registry
            .create("PV", config.clone())
            .map_err(to_py_err)?;
        let mut dwelling = self.acquire()?;
        let orientations = config
            .require_typed::<hares_equipment::PvConfig>("PV")
            .and_then(|typed| typed.array_orientations(&pv.name))
            .map_err(to_py_err)?;

        // PV init() validates that a SurfaceIrradiance entry exists for each
        // array. The environment computes real irradiance during simulation;
        // inject a placeholder so init() succeeds.
        let mut init_env = dwelling.latest_env().clone();
        for orientation in &orientations {
            dwelling
                .environment
                .check_pv_surface(orientation.surface_id)
                .map_err(to_py_err)?;
            if !init_env
                .weather
                .solar_irradiance
                .iter()
                .any(|s| s.surface_id == orientation.surface_id)
            {
                init_env.weather.solar_irradiance.push(SurfaceIrradiance {
                    surface_id: orientation.surface_id,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                });
            }
        }
        eq.init(&config, &init_env).map_err(to_py_err)?;

        dwelling.add_equipment(eq).map_err(to_py_err)?;
        // Only an accepted panel registers its orientations, so that Perez
        // irradiance is computed for them during simulation.
        for orientation in orientations {
            dwelling
                .environment
                .register_pv_surface(SurfaceGeometry {
                    surface_id: orientation.surface_id,
                    azimuth_deg: orientation.azimuth_deg,
                    tilt_deg: orientation.tilt_deg,
                    area_m2: 1.0,
                    omni_directional: false,
                })
                .map_err(to_py_err)?;
        }
        Ok(())
    }

    /// Add an EV with the driver actor that simulates its use. The driver
    /// joins with the EV; drivers and battery managers already attached to
    /// other equipment keep their state.
    pub fn add_ev(&mut self, ev: &PyEv) -> PyResult<()> {
        let config = ev_config_from_py(ev)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

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

        // Joins with the driver actor its seed builds.
        dwelling.add_equipment(eq).map_err(to_py_err)?;
        Ok(())
    }

    pub fn add_ev_with_driver(
        &mut self,
        vehicle_id: PyVehicleId,
        archetype_id: PyEvArchetypeId,
        seed: u64,
    ) -> PyResult<()> {
        use hares_core::actors::ev_driver::EvDriverActor;
        use hares_equipment::ev::catalog::{EvArchetypeId, L1_CAP_W, VehicleId};

        let rust_vid: VehicleId = vehicle_id.into();
        let rust_aid: EvArchetypeId = archetype_id.into();
        let spec = rust_vid.spec();
        let preset = rust_aid.preset();

        let max_power_w = match preset.charging_level {
            hares_types::ChargingLevel::L1 => spec.charger_power_w().min(L1_CAP_W),
            hares_types::ChargingLevel::L2 => spec.charger_power_w(),
        };

        let config = EquipmentConfig::from_typed(
            spec.label.to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_j: spec.capacity_j(),
                charging_level: Some(match preset.charging_level {
                    hares_types::ChargingLevel::L1 => "L1".to_string(),
                    hares_types::ChargingLevel::L2 => "L2".to_string(),
                }),
                max_charging_power_w: max_power_w,
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
                n_series: None,
                n_parallel: None,
                cell_resistance_ohm: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_w: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_w: None,
                chemistry: None,
                fuel_economy_j_per_m: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_w: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_va: None,
                cc_cv_transition_soc: None,
                charging_priority: None,
                discharge_respects_deadline: true,
                der_1547: None,
            },
        )
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

        let mut eq = self
            .equipment_registry
            .create("EV", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = self.acquire()?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;

        // The day-roll stream rides the reserved EV-driver stream nonce,
        // the same partition the registry path and the dwelling's built-in
        // drivers draw from; the driving sources' tagged keys are
        // stream-0 keys beside it - every consumer of the caller's key
        // draws from its own keystream. The caller's seed first mixes the
        // dwelling's identity (the config-stable building id): the same
        // seed added to two dwellings must not draw the same driving
        // series.
        let seed_bytes = hares_core::actors::ev_driver::driver_key_material(seed, dwelling.bldg_id);
        let day_roll = hares_core::rng::derive_sub_rng(
            &hares_core::ChaCha8Rng::from_seed(seed_bytes),
            hares_core::rng::RNG_STREAM_EV_DRIVER_BASE,
        );

        // The canonical built-in actor name ("EvDriver:<equipment>") takes
        // the place of the driver the EV's actor seed would build, now and
        // on every later rebuild of the built-in actors (set_tariff).
        let actor = EvDriverActor::new(
            &format!("EvDriver:{}", spec.label),
            spec.label,
            ev_driver_params(spec, preset, seed_bytes, max_power_w),
            day_roll,
        );

        dwelling
            .add_equipment_with_actors(eq, vec![Box::new(actor)])
            .map_err(to_py_err)?;
        Ok(())
    }

    pub fn add_protocol_bridge(&mut self, bridge: &PyProtocolBridge) -> PyResult<()> {
        let config = protocol_bridge_config_from_py(bridge)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let mut eq = self
            .equipment_registry
            .create("Protocol Bridge", config.clone())
            .map_err(to_py_err)?;
        let mut dwelling = self.acquire()?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;
        dwelling.add_equipment(eq).map_err(to_py_err)?;
        Ok(())
    }

    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.remove_equipment(name).map_err(to_py_err)?;
        Ok(())
    }

    /// Replace equipment by name with a new equipment config object (Battery, PV, or EV).
    ///
    /// Actors bound to the replaced name stay and drive the replacement
    /// when it keeps that name and accepts their signals; they are removed
    /// when the replacement takes another name or cannot accept their
    /// signals. A built-in EV driver or battery manager also stays only
    /// when the replacement is configured for it identically (an EV's
    /// capacity and charging, a battery's mode and power limits);
    /// otherwise a fresh one is built for the replacement. The
    /// replacement's own driver or battery manager is attached unless an
    /// actor already holds its name. A rejected replacement leaves the
    /// dwelling unchanged.
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
                    BatteryLutData::ChargingCurve(Box::new(lut.clone())),
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
                        BatteryLutData::ChargingCurve(Box::new(lut.clone())),
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

    /// Attaches a Python callable as the dwelling's batch callback: the
    /// recorder calls it with the batch's info and the batch as the
    /// zero-copy Arrow batch (pyo3-arrow's `PyRecordBatch`, the Arrow
    /// PyCapsule array interface over the batch's own buffers), once per
    /// flushed batch, in flush order. The step runs with the GIL
    /// released, so the call re-acquires it; an exception from the
    /// callable fails the run as a typed callback error with the raised
    /// object chained as `__cause__`, and the run stays ended after it.
    pub fn attach_callback(&mut self, report: &str, callback: Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            let repr = callback
                .repr()
                .map(|r| r.to_string())
                .unwrap_or_else(|_| "unprintable".to_string());
            return Err(PyTypeError::new_err(format!(
                "the batch callback must be callable, got {repr}"
            )));
        }
        let mut dwelling = self.acquire()?;
        dwelling
            .attach_callback(
                report,
                Box::new(crate::py_callback::PyBatchCallback::new(
                    callback.unbind(),
                    Arc::clone(&self.callback_cause),
                )),
            )
            .map_err(to_py_err)?;
        Ok(())
    }

    /// Freezes the observation plan for `fields` against this dwelling:
    /// every field resolved once to a typed source, failing on an unknown
    /// field and naming it. The gym environments build their plans at
    /// `reset()` and read every step through them.
    pub fn observation_plan(
        &self,
        fields: Vec<String>,
    ) -> PyResult<crate::py_gym::PyObservationPlan> {
        let dwelling = self.acquire()?;
        let plan = dwelling.observation_plan(&fields).map_err(to_py_err)?;
        Ok(crate::py_gym::PyObservationPlan {
            plan: std::sync::Arc::new(plan),
        })
    }

    /// Writes the planned observation into `out` (any object exposing a
    /// writable C-contiguous float64 buffer, a numpy row among them), in
    /// place, allocation-free: the plan's typed sources read the
    /// dwelling's live state directly, the values and NaN handling
    /// `to_observation_vec` reports on the same state.
    pub fn observe_into(
        &self,
        plan: &crate::py_gym::PyObservationPlan,
        out: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let buffer = PyBuffer::<f64>::get(out).map_err(|_| {
            PyValueError::new_err(
                "the observation row must expose a writable float64 buffer protocol view",
            )
        })?;
        if buffer.readonly() {
            return Err(PyValueError::new_err(
                "the observation row must be writable",
            ));
        }
        if buffer.item_count() != plan.plan.len() {
            return Err(PyValueError::new_err(format!(
                "the observation row holds {} slots for {} planned fields",
                buffer.item_count(),
                plan.plan.len()
            )));
        }
        match buffer.strides() {
            [stride] if *stride == size_of::<f64>() as isize => {}
            _ => {
                return Err(PyValueError::new_err(
                    "the observation row must be C-contiguous",
                ));
            }
        }
        // SAFETY: the buffer is writable, C-contiguous, holds the plan's
        // field count, and the PyBuffer keeps the export alive for the
        // synchronous write below.
        let out_slice =
            unsafe { slice::from_raw_parts_mut(buffer.buf_ptr().cast::<f64>(), plan.plan.len()) };
        let dwelling = self.acquire()?;
        dwelling
            .observe_into(&plan.plan, out_slice)
            .map_err(to_py_err)
    }

    pub fn reset_with_seed(&mut self, py: Python<'_>, seed: u64) -> PyResult<()> {
        // The reset reuses the blueprint's shared parses (the HPXML, the
        // weather and the defaults are the same `Arc`s), the rng derives
        // from the seed, and the generated schedule's diversity re-derives
        // under the seed's key: that re-derivation re-freezes the
        // blueprint, so the wrapper reports the rebuilt one (the one the
        // dwelling actually runs) and the settings carry the seed the
        // config reports.
        #[cfg(test)]
        TEST_BUILD_COUNT.fetch_add(1, Ordering::SeqCst);
        let mut config = self.config.clone();
        config.settings.sim_config.master_seed = seed;

        let dwelling = construct_detached(py, {
            let blueprint = Arc::clone(&self.blueprint);
            move || blueprint.build_with_seed(seed).map_err(to_py_err)
        })
        .map_err(|_| PyRuntimeError::new_err("reset_with_seed panicked"))?;
        self.blueprint = Arc::clone(dwelling.blueprint());
        self.dwelling = Mutex::new(dwelling);
        self.poisoned.store(false, Ordering::SeqCst);
        self.mutex_poison_events.store(0, Ordering::SeqCst);
        self.config = config;
        self.initialized = false;
        *self
            .zone_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.step_scratch = hares_core::StepResult::default();
        *self.callback_cause.lock().expect("cause slot mutex") = None;

        self.initialize()
    }

    /// Reseeds the dwelling's random streams in place, the streams
    /// construction assigns for `seed` with each at its start: the reset
    /// that restores a warmed-up state is `load_state(snapshot)` and then
    /// this — no re-construction, no second warm-up. The explicit full
    /// rebuild stays `reset_with_seed`.
    pub fn reseed(&mut self, seed: u64) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.reseed(seed).map_err(to_py_err)
    }

    pub fn save_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let dwelling = self.acquire()?;
        let bytes = serde_json::to_vec(&dwelling.save_checkpoint().map_err(to_py_err_display)?)
            .map_err(to_py_err_display)?;
        Ok(PyBytes::new(py, &bytes))
    }

    pub fn load_state(&self, state: &[u8]) -> PyResult<()> {
        let checkpoint = DwellingCheckpoint::deserialize_gated(state).map_err(to_py_err)?;
        let mut dwelling = self.acquire()?;
        dwelling.load_checkpoint(checkpoint).map_err(to_py_err)
    }

    pub fn save_checkpoint<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let dwelling = self.acquire()?;
        let bytes = serde_json::to_vec(&dwelling.save_checkpoint().map_err(to_py_err_display)?)
            .map_err(to_py_err_display)?;
        Ok(PyBytes::new(py, &bytes))
    }

    pub fn restore_building_state(&self, state: &[u8]) -> PyResult<()> {
        let checkpoint = DwellingCheckpoint::deserialize_gated(state).map_err(to_py_err)?;
        let mut dwelling = self.acquire()?;
        dwelling
            .restore_building_state(&checkpoint)
            .map_err(to_py_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "Dwelling(bldg_id={}, initialized={})",
            self.config.settings.bldg_id, self.initialized
        )
    }

    pub fn set_solar_override(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let solar_data = parse_solar_override(py, data)?;
        // Loud on non-finite irradiance, at the boundary that received
        // the value: never relying on the downstream screens. This is
        // the documented external-pipeline channel (pvlib/PySAM
        // injection — see
        // tests/python/generate_pvlib_solar_override.py), where a NaN
        // riding in from missing satellite data is the normal failure
        // mode. The walk runs on the parsed table, not on any arm's
        // extraction internals, so it covers every parse arm (path,
        // list, polars/pandas DataFrame, ndarray, dict) by construction.
        for (step, surfaces) in solar_data.iter().enumerate() {
            for (surface, s) in surfaces.iter().enumerate() {
                for (field, value) in [
                    ("direct_w_m2", s.direct_w_m2),
                    ("diffuse_w_m2", s.diffuse_w_m2),
                    ("reflected_w_m2", s.reflected_w_m2),
                    ("angle_of_incidence_rad", s.angle_of_incidence_rad),
                ] {
                    if !value.is_finite() {
                        return Err(PyValueError::new_err(format!(
                            "solar override '{field}' at timestep {step}, surface index \
                             {surface} is non-finite ({value}): a non-finite irradiance \
                             silently poisons the solar gains in release builds (the \
                             downstream finiteness screens are debug-only) — pass \
                             finite values or omit the entry"
                        )));
                    }
                }
            }
        }
        let mut dwelling = self.acquire()?;
        dwelling
            .environment
            .set_solar_override(solar_data)
            .map_err(to_py_err)
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
        crate::py_pv_sizing::pv_candidates_from_dwelling(
            &dwelling.roof_info,
            roof_shape,
            hares_physics::pv_sizing::PvRoofTuning {
                wall_azimuths: &dwelling.wall_azimuths,
                latitude: dwelling.latitude_deg,
                panel_watts,
                panel_area_m2,
                diffuse_fraction,
                roof_shape_user_override: user_override,
            },
            dwelling.pv_panel_defaults(),
        )
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
    ///     inverter_kw_ac: Optional inverter AC power rating (kW). When
    ///         ``Some``, must be paired with ``max_dc_ac_ratio`` to apply
    ///         an inverter-side DC capacity clamp (max DC = AC × ratio).
    ///     max_dc_ac_ratio: Optional maximum DC/AC oversizing ratio. When
    ///         ``Some``, must be paired with ``inverter_kw_ac`` to apply
    ///         the inverter DC-capacity clamp. Typical residential values
    ///         are 1.1–1.3 (NREL SAM default = 1.2).
    ///     main_panel_ampacity: Optional main electrical panel ampacity (amps).
    ///         When provided, the NEC 120% busbar backfeed rule is applied to
    ///         constrain the system to electrically-safe backfeed limits.
    ///         For example, a 100 A panel limits backfeed to ~4.8 kW AC;
    ///         a 200 A panel allows up to ~9.6 kW AC.
    ///     main_breaker_ampacity: Optional main breaker ampacity (amps). When
    ///         ``None`` (default), the main breaker is assumed to equal the
    ///         main panel ampacity. Override this for non-typical
    ///         configurations (e.g. 200 A panel with a 150 A main breaker).
    ///     roof_shape: Optional roof shape classification override. When
    ///         ``Some``, the provided shape is used directly and inference
    ///         is skipped. When ``None`` (default), roof shape is inferred
    ///         from building metadata.
    #[pyo3(signature = (*, target_w, min_w=2000.0, max_w=14000.0,
        diffuse_fraction=None, panel_watts=None, panel_area_m2=None,
        system_losses=None, inverter_ac_w=None, max_dc_ac_ratio=None,
        main_panel_ampacity=None, main_breaker_ampacity=None, roof_shape=None))]
    // PyO3 maps this keyword signature 1:1 to the Python API.
    #[allow(clippy::too_many_arguments)]
    pub fn estimate_pv_capacity(
        &self,
        target_w: f64,
        min_w: f64,
        max_w: f64,
        diffuse_fraction: Option<f64>,
        panel_watts: Option<u32>,
        panel_area_m2: Option<f64>,
        system_losses: Option<f64>,
        inverter_ac_w: Option<f64>,
        max_dc_ac_ratio: Option<f64>,
        main_panel_ampacity: Option<u32>,
        main_breaker_ampacity: Option<u32>,
        roof_shape: Option<crate::py_pv_sizing::PyRoofShape>,
    ) -> PyResult<crate::py_pv_sizing::PyPvSizingResult> {
        let mut dwelling = self.acquire()?;
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
        let result = crate::py_pv_sizing::size_pv_from_dwelling(
            &dwelling.roof_info,
            roof_shape,
            crate::py_pv_sizing::PvSizingTargets {
                target_w,
                min_w,
                max_w,
            },
            crate::py_pv_sizing::PvRoofSiting {
                wall_azimuths: &dwelling.wall_azimuths,
                latitude: dwelling.latitude_deg,
                diffuse_fraction,
                roof_shape_user_override: user_override,
            },
            hares_physics::pv_sizing::PvSystemTuning {
                system_losses,
                panel_watts,
                panel_area_m2,
                inverter_ac_w,
                max_dc_ac_ratio,
                main_panel_ampacity,
                main_breaker_ampacity,
            },
            dwelling.pv_panel_defaults(),
        )
        .map_err(pyo3::exceptions::PyValueError::new_err)?;
        if result.inner.main_panel_ampacity_missing {
            // The sizing's missing-ampacity condition is this dwelling's
            // own warning record (per run, every build): the sizing
            // proceeded without the NEC backfeed check.
            dwelling.warnings.push_warning(hares_types::Warning::new(
                "pv_sizing",
                "PV sizing proceeded without the electrical code check: no \
                 main_panel_ampacity provided",
            ));
        }
        Ok(result)
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
            .or_else(|| self.config.settings.sim_config.civil_timezone.clone())
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
            // The live billing fields are SI (J, W); the rates are tariff
            // data and keep their per-kWh units.
            cumulative_import_j: billing.cumulative_import_j(),
            cumulative_export_j: billing.cumulative_export_j(),
            peak_demand_w: billing.peak_demand_w(),
            cumulative_energy_cost_usd: billing.cumulative_energy_cost_usd(),
        }))
    }

    /// Drains and returns warning messages accumulated since the previous call.
    pub fn take_warnings(&self) -> PyResult<Vec<String>> {
        let mut dwelling = self.acquire()?;
        Ok(dwelling
            .take_warnings()
            .iter()
            .map(std::string::ToString::to_string)
            .collect())
    }

    /// Injects a warning message into the dwelling's warning buffer.
    pub fn push_warning(&self, msg: String) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.push_warning(msg);
        Ok(())
    }

    /// Returns the number of registered actors.
    pub fn actor_count(&self) -> PyResult<usize> {
        let dwelling = self.acquire()?;
        Ok(dwelling.actor_count())
    }

    /// Removes all equipment from the dwelling.
    pub fn clear_equipment(&self) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.clear_equipment().map_err(to_py_err)?;
        Ok(())
    }

    /// Removes all equipment whose end-use matches any of the given set.
    /// Returns the number of equipment pieces removed.
    #[pyo3(signature = (end_uses,))]
    pub fn remove_equipment_by_end_use(
        &self,
        end_uses: Vec<crate::py_enums::PyEndUse>,
    ) -> PyResult<usize> {
        let rust_end_uses: Vec<hares_types::EndUse> =
            end_uses.into_iter().map(From::from).collect();
        let mut dwelling = self.acquire()?;
        dwelling
            .remove_equipment_by_end_use(&rust_end_uses)
            .map_err(to_py_err)
    }

    /// Returns a snapshot of the current environment state.
    pub fn latest_env(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let dwelling = self.acquire()?;
        let env = dwelling.latest_env();
        let dict = PyDict::new(py);
        dict.set_item("outdoor_temp_c", env.weather.outdoor_temp_c)?;
        dict.set_item("outdoor_humidity_ratio", env.weather.outdoor_humidity_ratio)?;
        dict.set_item("ghi_w_m2", env.weather.ghi_w_m2)?;
        dict.set_item("dni_w_m2", env.weather.dni_w_m2)?;
        dict.set_item("dhi_w_m2", env.weather.dhi_w_m2)?;
        dict.set_item("pressure_kpa", env.weather.pressure_kpa)?;
        dict.set_item("wind_speed_m_s", env.weather.wind_speed_m_s)?;
        dict.set_item("mains_temp_c", env.weather.mains_temp_c)?;
        dict.set_item("grid_voltage_pu", env.grid.voltage_pu)?;
        dict.set_item("grid_frequency_hz", env.grid.frequency_hz)?;
        // Outage/islanding observability: `grid_voltage_pu` is the utility
        // service voltage (0.0 = outage); `grid_bus_voltage_pu` is the bus
        // voltage equipment actually sees (nonzero when a backup source
        // islands the home).
        dict.set_item("grid_bus_voltage_pu", env.grid.bus_voltage_pu())?;
        dict.set_item("grid_islanded", env.grid.islanded())?;
        // Islanded-imbalance accounting: residual load the island sources
        // could not cover / surplus generation the island could not absorb.
        // Both 0.0 during normal (grid-connected) operation.
        dict.set_item("island_unserved_w", dwelling.island_unserved_w())?;
        dict.set_item("island_excess_w", dwelling.island_excess_w())?;
        dict.set_item(
            "current_time",
            naive_chrono_to_py_datetime(py, env.current_time.naive_local())?,
        )?;
        let zones: Vec<Py<PyDict>> = env
            .zones
            .iter()
            .map(|z| {
                let zd = PyDict::new(py);
                zd.set_item("id", z.id.0)?;
                zd.set_item("temperature_c", z.temperature_c)?;
                zd.set_item("humidity_ratio", z.humidity_ratio)?;
                zd.set_item("volume_m3", z.volume_m3)?;
                Ok(zd.into())
            })
            .collect::<PyResult<_>>()?;
        dict.set_item("zones", zones)?;
        Ok(dict.into())
    }
}

/// The driver params an EV added through `add_ev_with_driver` is built
/// with: every behavioural field comes from the archetype preset or the
/// vehicle spec, never a local re-derivation. Free-standing so the
/// agreement tests below can pin the construction to the catalogue's fuel
/// economy and the seed path's away-charge power.
fn ev_driver_params(
    spec: &hares_equipment::ev::catalog::VehicleSpec,
    preset: &hares_equipment::ev::catalog::ArchetypePreset,
    seed_bytes: [u8; 32],
    max_power_w: f64,
) -> hares_core::actors::EvDriverParams {
    hares_core::actors::EvDriverParams {
        strategy: preset.strategy.clone(),
        plug_in_policy: preset.plug_in_policy.clone(),
        daily_drive_distance_m: preset.build_distance_schedule(seed_bytes),
        departure_time: preset.build_departure_schedule(seed_bytes),
        trip_duration: preset.build_duration_schedule(seed_bytes),
        arrival_time: preset.build_arrival_schedule(seed_bytes),
        event_day_ratio: preset.event_day_ratio,
        // The catalogue's datasheet figures are kWh/mi and kWh; the
        // driver's parameters are SI, converted once here with uom.
        fuel_economy_j_per_m: uom::si::f64::Energy::new::<uom::si::energy::kilowatt_hour>(
            spec.fuel_economy_kwh_per_mi,
        )
        .get::<uom::si::energy::joule>()
            / uom::si::f64::Length::new::<uom::si::length::mile>(1.0)
                .get::<uom::si::length::meter>(),
        capacity_j: uom::si::f64::Energy::new::<uom::si::energy::kilowatt_hour>(spec.capacity_kwh)
            .get::<uom::si::energy::joule>(),
        max_charge_w: max_power_w,
        average_speed_m_per_s: hares_core::actors::ev_driver::defaults::AVERAGE_SPEED_M_PER_S,
        range_anxiety_distance_m: hares_core::actors::ev_driver::defaults::RANGE_ANXIETY_DISTANCE_M,
        away_charge_fraction: 0.0,
        away_charge_power_w: uom::si::f64::Power::new::<uom::si::power::kilowatt>(
            preset.away_charge_power_kw,
        )
        .get::<uom::si::power::watt>(),
    }
}

impl PyDwelling {
    pub(crate) fn from_blueprint_build(
        dwelling: Dwelling,
        config: DwellingConfig,
        blueprint: Arc<DwellingBlueprint>,
    ) -> Self {
        Self {
            dwelling: Mutex::new(dwelling),
            poisoned: AtomicBool::new(false),
            mutex_poison_events: std::sync::atomic::AtomicU64::new(0),
            config,
            blueprint,
            initialized: false,
            registry: ActorRegistry::new(),
            equipment_registry: EquipmentRegistry::new(),
            zone_keys: Mutex::new(None),
            step_scratch: hares_core::StepResult::default(),
            gas_tariff: None,
            callback_cause: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Acquire the dwelling mutex after checking the poisoned flag.
    /// Returns `FatalDwellingError` if the dwelling is in a fatal error state.
    fn acquire(&self) -> PyResult<MutexGuard<'_, Dwelling>> {
        if crate::py_callback::delivery_in_progress() {
            return Err(reentry_refusal());
        }
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
    /// Returns `Err(StepFailure)` instead of `PyErr` so it can be used without holding the GIL:
    /// fatal errors carry `fatal: true` as data, so callers distinguish permanent from
    /// transient failures without reading message content.
    pub(crate) fn acquire_string(&self) -> Result<MutexGuard<'_, Dwelling>, StepFailure> {
        if crate::py_callback::delivery_in_progress() {
            return Err(StepFailure {
                msg: crate::py_callback::REENTRY_MESSAGE.to_string(),
                fatal: false,
            });
        }
        if self.poisoned.load(Ordering::Acquire) {
            return Err(StepFailure {
                msg: format!(
                    "{}dwelling is in a fatal error state (internal state corruption); create a new Dwelling instance",
                    FATAL_DWELLING_PREFIX
                ),
                fatal: true,
            });
        }
        self.dwelling.lock().map_err(|e| {
            let msg = format!(
                "{}dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
                FATAL_DWELLING_PREFIX, e
            );
            StepFailure { msg, fatal: true }
        })
    }

    /// Adds to the dwelling's run-total clamped-actions counter, the health
    /// record the batch and single-dwelling clip paths share.
    pub(crate) fn record_clamped_actions(&self, count: u64) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.record_clamped_actions(count).map_err(to_py_err)
    }

    /// GIL-free variant that preserves [`HaresError`] for proper Python exception mapping
    /// via [`to_py_err`].
    fn acquire_hares(&self) -> Result<MutexGuard<'_, Dwelling>, HaresError> {
        if crate::py_callback::delivery_in_progress() {
            return Err(reentry_hares_error());
        }
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

    /// String-error variant for use in GIL-free Rayon contexts
    /// (e.g. [`batch_step_py`]) where `HaresError` cannot cross thread boundaries
    /// as a `PyErr`.
    ///
    /// Routes through [`acquire_string`], so a fatal (poisoned) failure carries
    /// `fatal: true` as data, and [`batch_step_py`] distinguishes permanent from
    /// transient failures by that field. A [`step`] error is transient: the
    /// single-step path maps it through `map_run_error`, never
    /// `FatalDwellingError`, while the dwelling's poisoned flag is unset.
    pub(crate) fn step_core_string(&self) -> Result<hares_core::StepResult, StepFailure> {
        #[cfg(test)]
        if let Some(msg) = TEST_STEP_FAILURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Err(StepFailure { msg, fatal: false });
        }
        if !self.initialized {
            return Err(StepFailure {
                msg: "PyDwelling.step() called before initialize(). Call dwelling.initialize() \
                      first."
                    .to_string(),
                fatal: false,
            });
        }
        let mut dwelling = self.acquire_string()?;
        #[cfg(test)]
        if TEST_STEP_PANIC.load(Ordering::SeqCst) {
            // The guard is the dwelling's own MutexGuard: unwinding this
            // panic poisons the lock, the state the batch's panic branch
            // reads.
            panic!("injected step panic while the dwelling's lock is held");
        }
        dwelling.step().map_err(|e| StepFailure {
            msg: e.to_string(),
            fatal: false,
        })
    }

    /// The deadband range of the named equipment's thermostat class, or
    /// `None` when it takes no band.
    pub(crate) fn band_range_of(&self, name: &str) -> Result<Option<(f64, f64)>, String> {
        let dwelling = self.acquire_string().map_err(|failure| failure.msg)?;
        let eq = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| format!("equipment '{name}' not found"))?;
        Ok(eq
            .thermostat_band_class()
            .map(hares_types::ThermostatBandClass::range_c))
    }

    /// Convert a Python equipment config object (Battery, PV, or EV) to a boxed Equipment.
    fn py_any_to_equipment(
        &self,
        obj: &Bound<'_, PyAny>,
    ) -> PyResult<Box<dyn hares_equipment::Equipment>> {
        if let Ok(battery) = obj.extract::<PyRef<'_, PyBattery>>() {
            let config = battery_config_from_py(&battery)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
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
            let config = pv_config_from_py(&pv)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            return self
                .equipment_registry
                .create("PV", config)
                .map_err(to_py_err);
        }
        if let Ok(ev) = obj.extract::<PyRef<'_, PyEv>>() {
            let config = ev_config_from_py(&ev)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
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
            let config = protocol_bridge_config_from_py(&bridge)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
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
        // The timesteps iterator yields the steps' naive local wall clock,
        // the value the matching row's Time column carries.
        Ok(Some(naive_chrono_to_py_datetime(py, ts.naive_local())?))
    }

    fn __repr__(&self) -> String {
        format!(
            "TimestepsIter(step={}/{})",
            self.next_step, self.total_steps
        )
    }
}

/// The typed callback error a dwelling accessor raises when a batch
/// callback re-enters the dwelling from the delivery thread.
fn reentry_hares_error() -> HaresError {
    HaresError::Callback {
        report: hares_io::output::callback::TIMESTEP_REPORT.to_string(),
        message: crate::py_callback::REENTRY_MESSAGE.to_string(),
    }
}

/// The same refusal as [`reentry_hares_error`], mapped to the exception
/// the typed callback error carries.
fn reentry_refusal() -> PyErr {
    to_py_err(reentry_hares_error())
}

/// The shielding class's provenance label, in the form the read-back
/// records (the class's own spelling).
fn shielding_label(shielding: hares_physics::infiltration::ShieldingClass) -> String {
    match shielding {
        hares_physics::infiltration::ShieldingClass::Normal => "normal".to_string(),
        hares_physics::infiltration::ShieldingClass::Exposed => "exposed".to_string(),
        hares_physics::infiltration::ShieldingClass::WellShielded => "well-shielded".to_string(),
    }
}

/// The boundary kind's spelling in the read-back dict: the plan's kind
/// names, lowercased, the parser's `Other` kind carrying its own text.
fn boundary_kind_label(kind: &hares_io::hpxml::BoundaryType) -> String {
    match kind {
        hares_io::hpxml::BoundaryType::Wall => "wall".to_string(),
        hares_io::hpxml::BoundaryType::Roof => "roof".to_string(),
        hares_io::hpxml::BoundaryType::Floor => "floor".to_string(),
        hares_io::hpxml::BoundaryType::Window => "window".to_string(),
        hares_io::hpxml::BoundaryType::Skylight => "skylight".to_string(),
        hares_io::hpxml::BoundaryType::Door => "door".to_string(),
        hares_io::hpxml::BoundaryType::FoundationWall => "foundation wall".to_string(),
        hares_io::hpxml::BoundaryType::RimJoist => "rim joist".to_string(),
        hares_io::hpxml::BoundaryType::Slab => "slab".to_string(),
        hares_io::hpxml::BoundaryType::Other(text) => text.clone(),
    }
}

/// The resolved-parameter read-back as a dict of SI floats and strings,
/// the same values the Rust `resolved_model()` returns.
fn resolved_model_to_py(py: Python<'_>, model: &hares_core::ResolvedModel) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);

    let equipment = PyList::empty(py);
    for eq in &model.equipment {
        let entry = PyDict::new(py);
        entry.set_item("name", &eq.name)?;
        entry.set_item("id", eq.id.0)?;
        entry.set_item("end_use", eq.end_use.as_str())?;
        let parameters = PyList::empty(py);
        for parameter in &eq.parameters {
            let item = PyDict::new(py);
            item.set_item("name", parameter.name)?;
            item.set_item("value", parameter.value)?;
            item.set_item("unit", parameter.unit.symbol())?;
            parameters.append(item)?;
        }
        entry.set_item("parameters", parameters)?;
        entry.set_item(
            "heating_setpoint",
            eq.heating_setpoint
                .as_ref()
                .map(hares_io::SetpointSource::label),
        )?;
        entry.set_item(
            "cooling_setpoint",
            eq.cooling_setpoint
                .as_ref()
                .map(hares_io::SetpointSource::label),
        )?;
        equipment.append(entry)?;
    }
    dict.set_item("equipment", equipment)?;

    let boundaries = PyList::empty(py);
    for boundary in &model.boundaries {
        let entry = PyDict::new(py);
        entry.set_item("id", &boundary.id)?;
        entry.set_item("kind", boundary_kind_label(&boundary.kind))?;
        entry.set_item("area_m2", boundary.area_m2)?;
        entry.set_item("r_effective_m2k_w", boundary.r_effective_m2k_w)?;
        entry.set_item("films_m2k_w", boundary.films_m2k_w)?;
        entry.set_item("c_j_k", boundary.c_j_k)?;
        entry.set_item("same_zone", boundary.same_zone)?;
        entry.set_item("r_source", boundary.r_source.label())?;
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
    dict.set_item("boundaries", boundaries)?;

    let windows = PyList::empty(py);
    for window in &model.windows {
        let entry = PyDict::new(py);
        entry.set_item("id", &window.id)?;
        entry.set_item("area_m2", window.area_m2)?;
        entry.set_item("u_w_m2k", window.u_w_m2k)?;
        entry.set_item("shgc", window.shgc)?;
        windows.append(entry)?;
    }
    dict.set_item("windows", windows)?;

    let infiltration = PyDict::new(py);
    infiltration.set_item("ach50", model.infiltration.ach50)?;
    infiltration.set_item("ela_m2", model.infiltration.ela_m2)?;
    infiltration.set_item("constant_ach", model.infiltration.constant_ach)?;
    infiltration.set_item("height_m", model.infiltration.height_m)?;
    infiltration.set_item("has_flue", model.infiltration.has_flue)?;
    infiltration.set_item(
        "shielding",
        model.infiltration.shielding.map(shielding_label),
    )?;
    dict.set_item("infiltration", infiltration)?;

    let site = PyDict::new(py);
    site.set_item("latitude_deg", model.site.latitude_deg)?;
    site.set_item("longitude_deg", model.site.longitude_deg)?;
    site.set_item("elevation_m", model.site.elevation_m)?;
    site.set_item("utc_offset_h", model.site.utc_offset_h)?;
    dict.set_item("site", site)?;

    // The run's clock: the first step's local wall clock, the site offset,
    // the step and the simulated step count of the configured horizon - the
    // number a caller checks a report's row count against. The same
    // read-back `DwellingBlueprint.validate()` carries.
    let clock = PyDict::new(py);
    clock.set_item(
        "start",
        model.clock.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
    )?;
    clock.set_item("utc_offset_s", model.clock.utc_offset_s)?;
    clock.set_item("time_res_s", model.clock.time_res_s)?;
    clock.set_item("steps", model.clock.steps)?;
    dict.set_item("clock", clock)?;

    Ok(dict.into_any().unbind())
}

impl PyDwelling {
    /// Maps a run's error to a Python exception, chaining the attached
    /// callback's stored exception as `__cause__` on a typed callback
    /// error: the adapter stored the callable's `PyErr` when it failed
    /// the run, and the error that carries the failure out of the
    /// dwelling chains it. The slot is read without consuming: the
    /// engine's latch re-raises the ended run's error on every later
    /// call (a `finish()` after the failing step or close), and each
    /// re-raise chains the same original exception.
    fn map_run_error(&self, err: HaresError) -> PyErr {
        let py_err = to_py_err(err);
        Python::attach(|py| {
            let cause = self
                .callback_cause
                .lock()
                .expect("cause slot mutex")
                .as_ref()
                .map(|cause| cause.clone_ref(py));
            if let Some(cause) = cause {
                let _ = py_err.value(py).setattr("__cause__", cause.value(py));
            }
        });
        py_err
    }
}

pub(crate) fn to_py_err(err: HaresError) -> PyErr {
    let msg = err.to_string();
    // A rejected equipment's error carries the warnings it raised; the
    // exception class follows the reason it was rejected for.
    let classified = match &err {
        HaresError::RejectedEquipment { reason, .. } => reason.as_ref(),
        other => other,
    };
    match classified {
        // Io errors are mapped to ConfigError because they predominantly arise during
        // config/file loading (HPXML, weather, schedules). This is a conscious trade-off:
        // a rare runtime Io error will surface as ConfigError rather than adding a
        // separate Python exception type for a case that effectively never occurs.
        HaresError::Io(_)
        | HaresError::Dwelling(_)
        | HaresError::Envelope(_)
        | HaresError::ThermostatBand { .. }
        | HaresError::SolarOverrideMissingSurface { .. }
        | HaresError::MissingInput { .. }
        | HaresError::PreconditioningAxis { .. }
        // A run that cannot be exported for exact replay is the caller's
        // configuration mistake: the dwelling was changed in a way a
        // replay cannot rebuild.
        | HaresError::NotReplayable { .. } => HaresConfigError::new_err(msg),
        HaresError::Equipment(_) | HaresError::InvalidEquipmentParameter { .. } => {
            HaresEquipmentError::new_err(msg)
        }
        // The policy violation carries the variant's fields as attributes,
        // so a consumer that gates on health reads the event, the step,
        // the source, the count and the limit off the exception it caught.
        HaresError::HealthPolicyViolation {
            event,
            step_index,
            origin,
            count,
            limit,
        } => {
            let py_err = HaresHealthPolicyError::new_err(msg);
            Python::attach(|py| {
                let value = py_err.value(py);
                let _ = value.setattr("event", format!("{event:?}"));
                let _ = value.setattr("step_index", step_index);
                let _ = value.setattr("source", origin.as_str());
                let _ = value.setattr("count", count);
                let _ = value.setattr("limit", limit);
            });
            py_err
        }
        // Physics variant is DEPRECATED: zero production constructors remain in
        // the codebase (T-0144). Kept for serialization compatibility and future
        // hares-physics crate retrofitting (T-0145).
        HaresError::Callback { .. } => HaresCallbackError::new_err(msg),
        HaresError::Physics(_)
        | HaresError::Control(_)
        | HaresError::Tariff(_)
        | HaresError::Simulation(_)
        | HaresError::InvalidState(_)
        | HaresError::InvariantViolation { .. }
        | HaresError::NanDetected { .. }
        | HaresError::NegativeDeliveredEnergy { .. }
        | HaresError::RejectedEquipment { .. } => HaresSimulationError::new_err(msg),
    }
}

pub(crate) fn to_py_err_display<E: std::fmt::Display>(err: E) -> PyErr {
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
                    ed.set_item("end_use", obs.end_use.as_str())?;
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
                    .map(|(z, heat)| {
                        (
                            z.0,
                            heat.convective_w,
                            heat.radiant_w,
                            heat.shortwave_w,
                            heat.latent_w,
                        )
                    })
                    .collect::<Vec<_>>(),
            )?;
            pd.set_item("electrical_load_w", ports.electrical_load_w)?;
            pd.set_item("electrical_gen_w", ports.electrical_gen_w)?;
            pd.set_item("electrical_reactive_var", ports.electrical_reactive_var)?;
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
        d.set_item("window_through_glass_w", gains.window_through_glass_w)?;
        d.set_item("opaque_solar_lwr_w", gains.opaque_solar_lwr_w)?;
        d.set_item("interior_lwr_w", gains.interior_lwr_w)?;
        d.set_item("infiltration_w", gains.infiltration_w)?;
        d.set_item("ventilation_w", gains.ventilation_w)?;
        d.set_item("natural_ventilation_w", gains.natural_ventilation_w)?;
        #[cfg(feature = "observe")]
        {
            d.set_item("natural_ventilation_cw", gains.natural_ventilation_cw)?;
            d.set_item(
                "natural_ventilation_wind_angle_deg",
                gains.natural_ventilation_wind_angle_deg,
            )?;
            d.set_item(
                "natural_ventilation_q_stack_m3_s",
                gains.natural_ventilation_q_stack_m3_s,
            )?;
            d.set_item(
                "natural_ventilation_q_wind_m3_s",
                gains.natural_ventilation_q_wind_m3_s,
            )?;
            d.set_item(
                "natural_ventilation_cd_used",
                gains.natural_ventilation_cd_used,
            )?;
        }
        d.set_item("port_convective_w", gains.port_convective_w)?;
        d.set_item("port_radiant_w", gains.port_radiant_w)?;
        d.set_item("port_shortwave_w", gains.port_shortwave_w)?;
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
        // Non-finite floats are rejected at this boundary — the same
        // contract the equipment typed-config boundary (`from_typed`'s
        // finite walk) and the shared Python→JSON converter enforce.
        // Actor params feed BUILT-IN seeds (the IdealThermostat's
        // setpoints, the EvDriver's mileage/event-ratio/seed), so an
        // unguarded NaN arms a real actor with a poisoned value
        // (`with_heating_setpoint(NaN)`), and the failure surfaces only
        // mid-simulation: a telemetry panic in debug builds, a silently
        // never-acting controller behind a `tracing::error!` line in
        // release builds (the panic branch is cfg-gated). NaN comparisons
        // are always false, so the actor's conditionals silently stop
        // firing — the exact silent-substitution behavior the other
        // boundaries reject.
        if !f.is_finite() {
            return Err(PyValueError::new_err(format!(
                "actor parameter value ({f}) is non-finite: non-finite floats poison \
                 the actor's simulation-time behavior (NaN comparisons are always \
                 false, so its conditionals silently stop firing) — pass a finite \
                 value or omit the parameter"
            )));
        }
        return Ok(ConfigValue::Float(f));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(ConfigValue::Text(s));
    }
    if let Ok(arr) = value.extract::<Vec<f64>>() {
        // The same guard for list values: a non-finite element poisons
        // every consumer of the array parameter downstream (same class,
        // one deeper).
        if let Some((i, v)) = arr.iter().enumerate().find(|(_, v)| !v.is_finite()) {
            return Err(PyValueError::new_err(format!(
                "actor parameter list value [{i}] ({v}) is non-finite: non-finite \
                 floats poison the actor's simulation-time behavior — pass finite \
                 values or omit the element"
            )));
        }
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

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone, Timelike};
    use hares_equipment::pv::surface_id_for_orientation;
    use hares_equipment::{EquipmentRegistry, PvConfig};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
    };
    use pyo3::Bound;
    use pyo3::IntoPyObject;
    use pyo3::PyErr;
    use pyo3::Python;
    use pyo3::exceptions::PyValueError;
    use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyList};

    use super::pv_config_from_py;
    use super::{parse_solar_override_from_dict, parse_solar_override_from_list};
    use crate::py_equipment::PyPv;
    use crate::utils::extract_seconds;
    use crate::utils::{extract_datetime, parse_datetime_str};

    /// A rejected add or replace raises the exception class of the reason
    /// it was rejected for, and its message carries the rejected
    /// equipment's warnings.
    #[test]
    fn rejected_equipment_raises_the_exception_of_its_reason() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use hares_types::HaresError;
        let rejected = |reason: HaresError| HaresError::RejectedEquipment {
            reason: Box::new(reason),
            warnings: vec!["EV1: init warning".to_string()],
        };
        Python::attach(|py| {
            let equipment = super::to_py_err(rejected(HaresError::Equipment("dup".to_string())));
            assert!(equipment.is_instance_of::<super::HaresEquipmentError>(py));
            assert!(equipment.to_string().contains("EV1: init warning"));

            let config = super::to_py_err(rejected(HaresError::Io("no file".to_string())));
            assert!(config.is_instance_of::<super::HaresConfigError>(py));

            let simulation = super::to_py_err(rejected(HaresError::Control("taken".to_string())));
            assert!(simulation.is_instance_of::<super::HaresSimulationError>(py));
        });
    }

    /// A callback error converts to the callback error class, a subclass
    /// of the simulation error, naming the report in its message.
    #[test]
    fn callback_error_maps_to_python_error() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use hares_types::HaresError;
        Python::attach(|py| {
            let py_err = super::to_py_err(HaresError::Callback {
                report: "timestep".to_string(),
                message: "the store is down".to_string(),
            });
            assert!(
                py_err.is_instance_of::<super::HaresCallbackError>(py),
                "the callback error maps to the callback error class, got: {py_err}"
            );
            assert!(py_err.is_instance_of::<super::HaresSimulationError>(py));
            assert!(py_err.to_string().contains("timestep"));
        });
    }

    /// A health policy violation converts to the health policy error class,
    /// a subclass of the simulation error, whose attributes carry the
    /// variant's fields.
    #[test]
    fn health_policy_violation_maps_to_python_error() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use hares_types::{HaresError, HealthEvent};
        Python::attach(|py| {
            let violation = HaresError::HealthPolicyViolation {
                event: HealthEvent::PortRollback,
                step_index: Some(7),
                origin: "PortEq".to_string(),
                count: 1,
                limit: 0,
            };
            let py_err = super::to_py_err(violation);
            assert!(
                py_err.is_instance_of::<super::HaresHealthPolicyError>(py),
                "the violation maps to the health policy error, got: {py_err}"
            );
            assert!(py_err.is_instance_of::<super::HaresSimulationError>(py));
            let value = py_err.value(py);
            let attr = |name: &str| {
                value
                    .getattr(name)
                    .unwrap_or_else(|_| panic!("the exception carries a '{name}' attribute"))
            };
            assert_eq!(
                attr("event").extract::<String>().expect("event is a str"),
                "PortRollback"
            );
            assert_eq!(attr("step_index").extract::<u64>().expect("step_index"), 7);
            assert_eq!(
                attr("source").extract::<String>().expect("source is a str"),
                "PortEq"
            );
            assert_eq!(attr("count").extract::<u64>().expect("count"), 1);
            assert_eq!(attr("limit").extract::<u64>().expect("limit"), 0);
        });
    }

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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("not-a-date");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_datetime_str_rejects_invalid_month() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2019-13-01T00:00:00");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_datetime_str_naive_fractional_seconds() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2024-01-15T14:30:00.123456");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
            "2024-01-15T14:30:00.123456"
        );
        assert_eq!(dt.time().nanosecond(), 123_456_000);
    }

    #[test]
    fn test_parse_datetime_str_naive_single_digit_fractional() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2024-01-15T14:30:00.5");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.time().nanosecond(), 500_000_000);
    }

    #[test]
    fn test_parse_datetime_str_naive_zero_fractional() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2024-01-15T14:30:00.0");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.time().nanosecond(), 0);
    }

    #[test]
    fn test_parse_datetime_str_date_only_midnight_utc() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2024-01-15");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2024-01-15T00:00:00"
        );
    }

    #[test]
    fn test_parse_datetime_str_date_only_leap_day() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2024-02-29");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2024-02-29T00:00:00"
        );
    }

    #[test]
    fn test_parse_datetime_str_date_only_year_end() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = parse_datetime_str("2024-12-31");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2024-12-31T00:00:00"
        );
    }

    #[test]
    fn extract_datetime_handles_naive_datetime_with_microseconds() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let datetime_cls = py.import("datetime").unwrap().getattr("datetime").unwrap();
            // datetime.datetime(2024, 1, 15, 14, 30, 0, 123456)
            let obj = datetime_cls
                .call1((2024i32, 1i32, 15i32, 14i32, 30i32, 0i32, 123456i32))
                .unwrap();
            let result = extract_datetime(&obj);
            assert!(
                result.is_ok(),
                "naive datetime with microseconds should parse: {:?}",
                result
            );
            let dt = result.unwrap();
            assert_eq!(dt.time().nanosecond(), 123_456_000);
        });
    }

    #[test]
    fn extract_datetime_handles_naive_datetime_zero_microseconds() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let datetime_cls = py.import("datetime").unwrap().getattr("datetime").unwrap();
            // datetime.datetime(2024, 1, 15, 14, 30, 0)
            let obj = datetime_cls
                .call1((2024i32, 1i32, 15i32, 14i32, 30i32, 0i32))
                .unwrap();
            let result = extract_datetime(&obj);
            assert!(
                result.is_ok(),
                "naive datetime without microseconds should still parse: {:?}",
                result
            );
            let dt = result.unwrap();
            assert_eq!(dt.time().nanosecond(), 0);
        });
    }

    #[test]
    fn extract_datetime_handles_aware_datetime() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let datetime_mod = py.import("datetime").unwrap();
            let datetime_cls = datetime_mod.getattr("datetime").unwrap();
            let utc = datetime_mod
                .getattr("timezone")
                .unwrap()
                .getattr("utc")
                .unwrap();
            // datetime.datetime(2024, 1, 15, 14, 30, 0, tzinfo=datetime.timezone.utc)
            let obj = datetime_cls
                .call1((2024i32, 1i32, 15i32, 14i32, 30i32, 0i32, 0i32, &utc))
                .unwrap();
            let result = extract_datetime(&obj);
            assert!(
                result.is_ok(),
                "aware datetime should parse through RFC3339 path: {:?}",
                result
            );
            let dt = result.unwrap();
            assert_eq!(dt.time().nanosecond(), 0);
        });
    }

    fn pv_env(tilt_deg: f64, azimuth_deg: f64) -> EnvironmentState {
        let surface_id =
            surface_id_for_orientation(tilt_deg, azimuth_deg, 5.0).expect("surface id");
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pv = PyPv {
            name: "PV LUT".to_string(),
            capacity_w: 5000.0,
            tilt: 30.0,
            azimuth: 180.0,
            sam_lut_path: Some("/tmp/example.parquet".to_string()),
            soiling: None,
            der_1547: None,
        };
        let config = pv_config_from_py(&pv).expect("PV config");
        let typed: PvConfig = config.typed().expect("typed PvConfig");
        assert_eq!(typed.sam_lut_path.as_deref(), Some("/tmp/example.parquet"));
    }

    #[test]
    fn pypv_without_sam_lut_path_initializes_normally() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pv = PyPv {
            name: "PV No LUT".to_string(),
            capacity_w: 5000.0,
            tilt: 30.0,
            azimuth: 180.0,
            sam_lut_path: None,
            soiling: None,
            der_1547: None,
        };
        let config = pv_config_from_py(&pv).expect("PV config");
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|_py| {
            let dir = tempfile::tempdir().expect("temp dir");
            let bad_lut_path = dir.path().join("bad_pv_lut.txt");
            fs::write(&bad_lut_path, "not,a,valid,lut\n1,2,3,4").expect("write test file");
            let pv = PyPv {
                name: "PV Bad LUT".to_string(),
                capacity_w: 5000.0,
                tilt: 30.0,
                azimuth: 180.0,
                sam_lut_path: Some(bad_lut_path.display().to_string()),
                soiling: None,
                der_1547: None,
            };
            let config = pv_config_from_py(&pv).expect("PV config");
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
        });
    }

    // --------------------------------------------------------------------
    // Mutex poison flag tests
    // --------------------------------------------------------------------

    use super::FATAL_DWELLING_PREFIX;
    use super::StepFailure;

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

        fn acquire(&self) -> Result<MutexGuard<'_, i32>, StepFailure> {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(StepFailure {
                    msg: format!("{}fatal: state corruption", FATAL_DWELLING_PREFIX),
                    fatal: true,
                });
            }
            self.lock.lock().map_err(|_| StepFailure {
                msg: "acquire failed: poison".to_string(),
                fatal: true,
            })
        }

        fn mark_poisoned(&self) {
            self.poisoned.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn mutex_poison_detected_after_panic() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let guard = PoisonGuard::new();
        guard.mark_poisoned();
        assert!(guard.poisoned.load(Ordering::Acquire));
        let result = guard.acquire();
        assert!(result.is_err(), "acquire should fail when flag is set");
        let failure = result.unwrap_err();
        assert!(
            failure.fatal,
            "the poisoned flag's failure is fatal, carried as data"
        );
        assert!(
            failure.msg.contains("fatal: state corruption"),
            "the message states the corruption: {}",
            failure.msg
        );
    }

    #[test]
    fn acquire_fails_on_poisoned_mutex_even_without_flag() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fn a_fatal_failure_carries_its_classification_as_data() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let guard = PoisonGuard::new();
        guard.mark_poisoned();
        let err = guard.acquire().unwrap_err();
        assert!(
            err.fatal,
            "the fatal failure is flagged by its field, readable without \
             the message"
        );
        assert!(
            err.msg.starts_with(FATAL_DWELLING_PREFIX),
            "the sentinel text stays in the fatal message for its readers"
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

        fn acquire_string(&self) -> Result<MutexGuard<'_, i32>, StepFailure> {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(StepFailure {
                    msg: format!("{}fatal: state corruption", FATAL_DWELLING_PREFIX),
                    fatal: true,
                });
            }
            self.lock.lock().map_err(|_| StepFailure {
                msg: "acquire failed".to_string(),
                fatal: true,
            })
        }

        fn mark_poisoned(&self) {
            self.poisoned.store(true, Ordering::SeqCst);
        }

        /// Mirrors `PyDwelling::step_core_string`: locks via `acquire_string`,
        /// then performs a step. Fatal errors carry `fatal: true` as data.
        fn step_core_string(&self) -> Result<i32, StepFailure> {
            let _guard = self.acquire_string()?;
            Ok(42)
        }
    }

    #[test]
    fn step_core_string_propagates_the_fatal_flag_when_poisoned() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let guard = StepGuard::new();
        guard.mark_poisoned();
        let err = guard.step_core_string().unwrap_err();
        assert!(
            err.fatal,
            "step_core_string must propagate the fatal flag from acquire_string \
             so batch_step_py can distinguish permanent from transient failures"
        );
    }

    #[test]
    fn step_core_string_succeeds_when_not_poisoned() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let guard = StepGuard::new();
        let result = guard.step_core_string();
        assert_eq!(result.unwrap(), 42);
    }

    fn make_valid_solar_dict<'py>(py: Python<'py>) -> Bound<'py, PyDict> {
        let dict = PyDict::new(py);
        let surface = PyDict::new(py);
        surface.set_item("direct", vec![100.0_f64]).unwrap();
        surface.set_item("diffuse", vec![50.0_f64]).unwrap();
        surface.set_item("reflected", vec![25.0_f64]).unwrap();
        surface.set_item("aoi", vec![0.5_f64]).unwrap();
        dict.set_item(1_u32, surface).unwrap();
        dict
    }

    #[test]
    fn solar_dict_valid_produces_correct_result() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let dict = make_valid_solar_dict(py);
            let result = parse_solar_override_from_dict(py, dict.as_any()).unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].len(), 1);
            assert_eq!(result[0][0].surface_id, 1);
            assert!((result[0][0].direct_w_m2 - 100.0).abs() < 1e-9);
            assert!((result[0][0].diffuse_w_m2 - 50.0).abs() < 1e-9);
            assert!((result[0][0].reflected_w_m2 - 25.0).abs() < 1e-9);
            assert!((result[0][0].angle_of_incidence_rad - 0.5).abs() < 1e-9);
        });
    }

    fn to_py_value_string(err: PyErr, py: Python<'_>) -> String {
        err.value(py).to_string()
    }

    #[test]
    fn solar_dict_missing_direct_key_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let dict = PyDict::new(py);
            let surface = PyDict::new(py);
            surface.set_item("diffuse", vec![50.0_f64]).unwrap();
            surface.set_item("reflected", vec![25.0_f64]).unwrap();
            surface.set_item("aoi", vec![0.5_f64]).unwrap();
            dict.set_item(1_u32, surface).unwrap();

            let err = parse_solar_override_from_dict(py, dict.as_any()).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn solar_dict_missing_diffuse_key_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let dict = PyDict::new(py);
            let surface = PyDict::new(py);
            surface.set_item("direct", vec![100.0_f64]).unwrap();
            surface.set_item("reflected", vec![25.0_f64]).unwrap();
            surface.set_item("aoi", vec![0.5_f64]).unwrap();
            dict.set_item(1_u32, surface).unwrap();

            let err = parse_solar_override_from_dict(py, dict.as_any()).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn solar_dict_missing_reflected_key_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let dict = PyDict::new(py);
            let surface = PyDict::new(py);
            surface.set_item("direct", vec![100.0_f64]).unwrap();
            surface.set_item("diffuse", vec![50.0_f64]).unwrap();
            surface.set_item("aoi", vec![0.5_f64]).unwrap();
            dict.set_item(1_u32, surface).unwrap();

            let err = parse_solar_override_from_dict(py, dict.as_any()).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn solar_dict_missing_aoi_key_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let dict = PyDict::new(py);
            let surface = PyDict::new(py);
            surface.set_item("direct", vec![100.0_f64]).unwrap();
            surface.set_item("diffuse", vec![50.0_f64]).unwrap();
            surface.set_item("reflected", vec![25.0_f64]).unwrap();
            dict.set_item(1_u32, surface).unwrap();

            let err = parse_solar_override_from_dict(py, dict.as_any()).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn solar_list_valid_produces_correct_result() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let surface_dict = PyDict::new(py);
            surface_dict.set_item("surface_id", 1_u32).unwrap();
            surface_dict.set_item("direct_w_m2", 100.0_f64).unwrap();
            surface_dict.set_item("diffuse_w_m2", 50.0_f64).unwrap();
            surface_dict.set_item("reflected_w_m2", 25.0_f64).unwrap();

            let step_list = PyList::new(py, [surface_dict.as_any()]).unwrap();
            let top_list = PyList::new(py, [step_list.as_any()]).unwrap();

            let result = parse_solar_override_from_list(py, &top_list).unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].len(), 1);
            assert_eq!(result[0][0].surface_id, 1);
            assert!((result[0][0].direct_w_m2 - 100.0).abs() < 1e-9);
            assert!((result[0][0].diffuse_w_m2 - 50.0).abs() < 1e-9);
            assert!((result[0][0].reflected_w_m2 - 25.0).abs() < 1e-9);
        });
    }

    #[test]
    fn solar_list_missing_surface_id_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let surface_dict = PyDict::new(py);
            surface_dict.set_item("direct_w_m2", 100.0_f64).unwrap();
            surface_dict.set_item("diffuse_w_m2", 50.0_f64).unwrap();
            surface_dict.set_item("reflected_w_m2", 25.0_f64).unwrap();

            let step_list = PyList::new(py, [surface_dict.as_any()]).unwrap();
            let top_list = PyList::new(py, [step_list.as_any()]).unwrap();

            let err = parse_solar_override_from_list(py, &top_list).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("surface_id"), "got: {msg}");
        });
    }

    #[test]
    fn solar_list_missing_direct_w_m2_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let surface_dict = PyDict::new(py);
            surface_dict.set_item("surface_id", 1_u32).unwrap();
            surface_dict.set_item("diffuse_w_m2", 50.0_f64).unwrap();
            surface_dict.set_item("reflected_w_m2", 25.0_f64).unwrap();

            let step_list = PyList::new(py, [surface_dict.as_any()]).unwrap();
            let top_list = PyList::new(py, [step_list.as_any()]).unwrap();

            let err = parse_solar_override_from_list(py, &top_list).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn solar_list_missing_diffuse_w_m2_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let surface_dict = PyDict::new(py);
            surface_dict.set_item("surface_id", 1_u32).unwrap();
            surface_dict.set_item("direct_w_m2", 100.0_f64).unwrap();
            surface_dict.set_item("reflected_w_m2", 25.0_f64).unwrap();

            let step_list = PyList::new(py, [surface_dict.as_any()]).unwrap();
            let top_list = PyList::new(py, [step_list.as_any()]).unwrap();

            let err = parse_solar_override_from_list(py, &top_list).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn solar_list_missing_reflected_w_m2_returns_valueerror() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let surface_dict = PyDict::new(py);
            surface_dict.set_item("surface_id", 1_u32).unwrap();
            surface_dict.set_item("direct_w_m2", 100.0_f64).unwrap();
            surface_dict.set_item("diffuse_w_m2", 50.0_f64).unwrap();

            let step_list = PyList::new(py, [surface_dict.as_any()]).unwrap();
            let top_list = PyList::new(py, [step_list.as_any()]).unwrap();

            let err = parse_solar_override_from_list(py, &top_list).unwrap_err();
            assert!(err.is_instance_of::<PyValueError>(py));
            let msg = to_py_value_string(err, py);
            assert!(msg.contains("missing required key"), "got: {msg}");
        });
    }

    #[test]
    fn extract_seconds_rejects_inf() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let test_values = [f64::INFINITY, f64::NEG_INFINITY, f64::NAN];
            for val in test_values {
                let obj = val.into_pyobject(py).unwrap();
                let err = extract_seconds(obj.as_any()).unwrap_err();
                assert!(
                    err.is_instance_of::<PyValueError>(py),
                    "expected PyValueError for {val}"
                );
                let msg = to_py_value_string(err, py);
                assert!(
                    msg.contains("must be finite"),
                    "expected 'must be finite' in error for {val}, got: {msg}"
                );
            }
        });
    }

    #[test]
    fn extract_seconds_rejects_overflow() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let test_values = [1e20_f64, -1e20_f64];
            for val in test_values {
                let obj = val.into_pyobject(py).unwrap();
                let err = extract_seconds(obj.as_any()).unwrap_err();
                assert!(
                    err.is_instance_of::<PyValueError>(py),
                    "expected PyValueError for {val}"
                );
                let msg = to_py_value_string(err, py);
                assert!(
                    msg.contains("too large for i64"),
                    "expected 'too large for i64' in error for {val}, got: {msg}"
                );
            }
        });
    }

    #[test]
    fn extract_seconds_accepts_negative_finite_f64() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let cases: [(f64, i64); 2] = [(-100.0, -100), (-1e15_f64, -1_000_000_000_000_000)];
            for (input, expected) in cases {
                let obj = input.into_pyobject(py).unwrap();
                let result = extract_seconds(obj.as_any()).unwrap();
                assert_eq!(
                    result, expected,
                    "extract_seconds({input}) = {result}, expected {expected}"
                );
            }
        });
    }

    #[test]
    fn extract_seconds_returns_correct_i64_for_finite_f64() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let cases: [(f64, i64); 5] =
                [(3600.0, 3600), (3600.7, 3601), (2.3, 2), (2.7, 3), (0.0, 0)];
            for (input, expected) in cases {
                let obj = input.into_pyobject(py).unwrap();
                let result = extract_seconds(obj.as_any()).unwrap();
                assert_eq!(
                    result, expected,
                    "extract_seconds({input}) = {result}, expected {expected}"
                );
            }
        });
    }

    /// The seed path's away-charge default: `actor_registry` builds an
    /// EvDriver whose `away_charge_power_w` falls back to this when the
    /// config carries no override.
    const SEED_PATH_AWAY_CHARGE_POWER_W: f64 = 6600.0;

    #[test]
    fn add_ev_with_driver_away_charge_power_matches_the_seed_path() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use hares_control::DispatchRequest;
        use hares_core::Actor;
        use hares_core::actors::ev_driver::EvDriverActor;
        use hares_equipment::ev::catalog::{EvArchetypeId, VehicleId};
        use hares_types::{ControlSignal, ScheduleSource};
        use rand::SeedableRng;

        let spec = VehicleId::ChevyBoltEv.spec();
        for aid in EvArchetypeId::ALL {
            let params = super::ev_driver_params(spec, aid.preset(), [7u8; 32], 7200.0);
            assert_eq!(
                params.away_charge_power_w, SEED_PATH_AWAY_CHARGE_POWER_W,
                "{aid}: the Python-built driver's away-charge power must be the seed path's default"
            );
        }

        // Behavioral half: with `away_charge_fraction` set above 0.0 (the
        // condition the two paths disagree under), the driver built from
        // `add_ev_with_driver`'s params dispatches its away session at the
        // preset's power, which the equipment mirrors as
        // `away_charge_power_kw` telemetry. Constant schedules pin the
        // trip to departure minute 0, 600 driven minutes, arrival at 600.
        let mut params = super::ev_driver_params(
            spec,
            EvArchetypeId::DailyCommuterL2.preset(),
            [7u8; 32],
            7200.0,
        );
        params.away_charge_fraction = 0.3;
        params.event_day_ratio = 1.0;
        params.daily_drive_distance_m = ScheduleSource::Constant(48_280.32);
        params.departure_time = ScheduleSource::Constant(0.0);
        params.trip_duration = ScheduleSource::Constant(600.0);
        params.arrival_time = None;
        let mut actor = EvDriverActor::new(
            "EvDriver:EV1",
            "EV1",
            params,
            hares_core::ChaCha8Rng::from_seed([7u8; 32]),
        );

        let start = FixedOffset::east_opt(0)
            .expect("UTC offset")
            .with_ymd_and_hms(2026, 6, 21, 0, 0, 0)
            .single()
            .expect("valid start");
        let mut away_powers: Vec<f64> = Vec::new();
        for minute in 0..=1440i64 {
            let mut env = pv_env(30.0, 180.0);
            env.current_time = start + ChronoDuration::minutes(minute);
            let mut out: Vec<DispatchRequest> = Vec::new();
            actor.decide(&env, &mut out);
            for req in out {
                if let ControlSignal::EvAwayCharge { power_w } = req.signal {
                    away_powers.push(power_w);
                }
            }
        }
        assert!(
            !away_powers.is_empty(),
            "the pinned trip must produce an away-charging session"
        );
        assert!(
            away_powers
                .iter()
                .all(|p| (*p - SEED_PATH_AWAY_CHARGE_POWER_W).abs() < 1e-9),
            "every away-charge dispatch must carry the seed path's 6600 W, got {away_powers:?}"
        );
    }

    #[test]
    fn add_ev_with_driver_fuel_economy_matches_the_catalogue() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use hares_equipment::ev::catalog::{EvArchetypeId, VehicleId};

        let spec = VehicleId::ChevyBoltEv.spec();
        let quotient = spec.capacity_kwh / spec.range_miles;
        // The catalogue's kWh/mi and the driver's J/m: the same figure
        // through the one uom conversion the builder performs.
        let catalogue_j_per_m = uom::si::f64::Energy::new::<uom::si::energy::kilowatt_hour>(
            spec.fuel_economy_kwh_per_mi,
        )
        .get::<uom::si::energy::joule>()
            / uom::si::f64::Length::new::<uom::si::length::mile>(1.0)
                .get::<uom::si::length::meter>();
        for aid in EvArchetypeId::ALL {
            let params = super::ev_driver_params(spec, aid.preset(), [7u8; 32], 7200.0);
            assert_eq!(
                params.fuel_economy_j_per_m, catalogue_j_per_m,
                "{aid}: the Python-built driver's fuel economy must be the catalogue's"
            );
            assert_ne!(
                params.fuel_economy_j_per_m, quotient,
                "{aid}: the catalogue figure must not be the capacity/range quotient"
            );
        }
    }
}

/// `profiling_summary_lists_actors`: the per-actor
/// totals reach the Python `profiling_summary()` dict in registration
/// order, accumulated over the run. The probes are defined here because
/// hares-core's test actors are test items another crate cannot import;
/// the whole module compiles only under `test-profiling`, which enables
/// the crate's `profiling` feature.
#[cfg(all(test, feature = "profiling"))]
mod profiling_summary_tests {
    use std::path::PathBuf;

    use hares_control::DispatchRequest;
    use hares_core::Actor;
    use hares_types::EnvironmentState;
    use pyo3::Python;
    use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyList, PyListMethods, PyString};

    use super::{PyDwelling, to_py_err};

    /// No interests (the filter calls `decide()` every step) and dispatches
    /// nothing.
    struct ProbeActor {
        name: &'static str,
    }

    use hares_types::TelemetryField;

    impl Actor for ProbeActor {
        fn name(&self) -> &str {
            self.name
        }
        fn output_variables(&self) -> &[TelemetryField] {
            &[]
        }

        fn decide(&mut self, _env: &EnvironmentState, _out: &mut Vec<DispatchRequest>) {}
    }

    #[test]
    fn profiling_summary_lists_actors() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            root.push("../..");
            let fixture = root.join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
            let defaults = root.join("python/hares/defaults");

            let sim_config = crate::py_config::PySimulationConfig::new(
                "2023-01-01T00:00:00-07:00",
                crate::py_config::WholeSeconds(96 * 3600),
                crate::py_config::WholeSeconds(3600),
                None,
                false,
                false,
                hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
                None,
                0,
                None,
                "on",
                None,
                None,
                hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                None,
                crate::py_config::PyWarmupArg::Absent,
                "hour24",
                "typical",
                None,
            )
            .expect("the sim config builds");

            let mut dwelling = PyDwelling::from_hpxml(
                &py.get_type::<PyDwelling>(),
                PyString::new(py, fixture.join("building.xml").to_str().expect("utf8")).as_any(),
                Some(
                    PyString::new(py, fixture.join("schedule.csv").to_str().expect("utf8"))
                        .as_any(),
                ),
                PyString::new(py, fixture.join("weather.epw").to_str().expect("utf8")).as_any(),
                sim_config,
                1,
                Some(PyString::new(py, defaults.to_str().expect("utf8")).as_any()),
                None,
                None,
            )
            .expect("cz2a fixture must load");

            // Add the probes through the inner Dwelling, in that order.
            {
                let mut d = dwelling
                    .acquire()
                    .expect("fresh dwelling must not be poisoned");
                d.add_actor(Box::new(ProbeActor { name: "probe_a" }))
                    .map_err(to_py_err)
                    .expect("probe_a must register");
                d.add_actor(Box::new(ProbeActor { name: "probe_b" }))
                    .map_err(to_py_err)
                    .expect("probe_b must register");
            }

            dwelling.initialize().expect("initialize must succeed");
            for _ in 0..96 {
                dwelling.step(py).expect("cz2a fixture step must succeed");
            }

            let summary = dwelling
                .profiling_summary(py)
                .expect("a profiling build reports a summary");
            let dict = summary
                .bind(py)
                .cast::<PyDict>()
                .expect("profiling_summary returns a dict");
            let per_actor = dict
                .get_item("per_actor")
                .expect("get_item must not raise")
                .expect("the summary must carry a per_actor key")
                .cast::<PyList>()
                .expect("per_actor must be a list")
                .clone();
            // (name, calls) per entry, in the dict's registration order.
            let listed: Vec<(String, u64)> = per_actor
                .iter()
                .map(|entry| {
                    let entry = entry
                        .cast::<PyDict>()
                        .expect("per_actor entry must be a dict");
                    let name: String = entry
                        .get_item("name")
                        .expect("get_item must not raise")
                        .expect("entry must carry a name")
                        .extract()
                        .expect("name must be a str");
                    let calls: u64 = entry
                        .get_item("calls")
                        .expect("get_item must not raise")
                        .expect("entry must carry calls")
                        .extract()
                        .expect("calls must be an int");
                    (name, calls)
                })
                .collect();
            let position =
                |name: &str| listed.iter().position(|(entry_name, _)| entry_name == name);
            let probe_a = position("probe_a");
            let probe_b = position("probe_b");
            assert!(
                probe_a.is_some() && probe_b.is_some(),
                "per_actor must hold both probes: {listed:?}"
            );
            assert!(
                probe_a.unwrap() < probe_b.unwrap(),
                "probe_a must be listed before probe_b (registration order): {listed:?}"
            );
            for (name, calls) in listed
                .iter()
                .filter(|(n, _)| n == "probe_a" || n == "probe_b")
            {
                assert_eq!(
                    calls, &96,
                    "'{name}' declares no interests, so the filter must call decide() every step"
                );
            }
        });
    }

    /// The Python summary's `finish_s` is None until the run's finalising
    /// call has run and a float after it: a summary taken mid-run never
    /// shows a zero that reads as a measurement.
    #[test]
    fn profiling_summary_reports_finish() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| {
            let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            root.push("../..");
            let fixture = root.join("tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh");
            let defaults = root.join("python/hares/defaults");

            let sim_config = crate::py_config::PySimulationConfig::new(
                "2023-01-01T00:00:00-07:00",
                crate::py_config::WholeSeconds(96 * 3600),
                crate::py_config::WholeSeconds(3600),
                None,
                false,
                false,
                hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
                None,
                0,
                None,
                "on",
                None,
                None,
                hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                None,
                crate::py_config::PyWarmupArg::Absent,
                "hour24",
                "typical",
                None,
            )
            .expect("the sim config builds");

            let mut dwelling = PyDwelling::from_hpxml(
                &py.get_type::<PyDwelling>(),
                PyString::new(py, fixture.join("building.xml").to_str().expect("utf8")).as_any(),
                Some(
                    PyString::new(py, fixture.join("schedule.csv").to_str().expect("utf8"))
                        .as_any(),
                ),
                PyString::new(py, fixture.join("weather.epw").to_str().expect("utf8")).as_any(),
                sim_config,
                1,
                Some(PyString::new(py, defaults.to_str().expect("utf8")).as_any()),
                None,
                None,
            )
            .expect("cz2a fixture must load");

            dwelling.initialize().expect("initialize must succeed");
            for _ in 0..96 {
                dwelling.step(py).expect("cz2a fixture step must succeed");
            }

            let finish_key = |dwelling: &PyDwelling| {
                let summary = dwelling
                    .profiling_summary(py)
                    .expect("a profiling build reports a summary");
                summary
                    .bind(py)
                    .get_item("finish_s")
                    .expect("get_item must not raise")
            };
            assert!(
                finish_key(&dwelling).is_none(),
                "the summary of an unfinalised run reports finish_s as None"
            );

            // The finalising call runs on the inner dwelling: the binding's
            // own simulate() builds the results frame after finalising,
            // which this test's interpreter need not carry. The subject is
            // the timer the engine sets, read here through the Python
            // summary.
            {
                let mut d = dwelling
                    .acquire()
                    .expect("fresh dwelling must not be poisoned");
                d.simulate().expect("the finalising simulate completes");
            }
            let finish = finish_key(&dwelling)
                .extract::<f64>()
                .expect("finish_s is a float after the finalising call");
            assert!(finish.is_finite(), "the measurement is a float: {finish}");
        });
    }
}

/// The construction GIL discipline: the four Python-facing constructors run
/// the expensive parse+resolve detached, so parallel callers construct
/// concurrently, and no Python-owned state is touched while released.
#[cfg(test)]
mod construction_gil_tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use arrow::array::{Float64Array, RecordBatch};
    use pyo3::Python;
    use pyo3::types::PyString;

    use crate::py_blueprint::PyDwellingBlueprint;
    use crate::py_config::{PySimulationConfig, WholeSeconds};
    use crate::py_dwelling::{CONSTRUCT_GIL_STATE, PyDwelling};

    const THREADS: usize = 4;
    /// Timed repetitions per scenario; the minimum is compared, so a loaded
    /// machine inflates both sides and the comparison stays about the
    /// construction cost, not scheduler noise.
    const ATTEMPTS: usize = 3;

    fn fixture_paths() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
        (
            fixture.join("building.xml"),
            fixture.join("schedule.csv"),
            fixture.join("weather.epw"),
            root.join("python/hares/defaults"),
        )
    }

    fn sim_config_1h_900s() -> PySimulationConfig {
        PySimulationConfig::new(
            "2023-01-01T00:00:00-07:00",
            WholeSeconds(3600),
            WholeSeconds(900),
            None,
            false,
            false,
            hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
            None,
            0,
            None,
            "on",
            None,
            None,
            hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            None,
            crate::py_config::PyWarmupArg::Absent,
            "hour24",
            "typical",
            None,
        )
        .expect("the sim config builds")
    }

    fn construct_one() {
        let (hpxml, schedule, weather, defaults) = fixture_paths();
        Python::attach(|py| {
            let cls = py.get_type::<PyDwelling>();
            PyDwelling::from_hpxml(
                &cls,
                PyString::new(py, hpxml.to_str().expect("utf8")).as_any(),
                Some(PyString::new(py, schedule.to_str().expect("utf8")).as_any()),
                PyString::new(py, weather.to_str().expect("utf8")).as_any(),
                sim_config_1h_900s(),
                1,
                Some(PyString::new(py, defaults.to_str().expect("utf8")).as_any()),
                None,
                None,
            )
            .expect("the cz4a fixture must construct");
        });
    }

    /// The concurrent-construction probe: N threads each constructing one
    /// dwelling, against the wall time the same N constructions take
    /// serially. While construction holds the GIL throughout, the threads
    /// serialise and the parallel wall time is the serial one (the recorded
    /// RED: 1.02x at the base); the assertion pins the speedup the release
    /// buys, above scheduler noise.
    #[test]
    fn concurrent_construction_scales() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Warm the file cache once, untimed, so the probe times parse+resolve.
        construct_one();

        let mut serial_best = std::time::Duration::MAX;
        for _ in 0..ATTEMPTS {
            let start = Instant::now();
            for _ in 0..THREADS {
                construct_one();
            }
            serial_best = serial_best.min(start.elapsed());
        }

        let mut parallel_best = std::time::Duration::MAX;
        for _ in 0..ATTEMPTS {
            let start = Instant::now();
            std::thread::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(construct_one);
                }
            });
            parallel_best = parallel_best.min(start.elapsed());
        }

        let speedup = serial_best.as_secs_f64() / parallel_best.as_secs_f64();
        eprintln!(
            "concurrent construction: serial (best of {ATTEMPTS}) {serial_best:?}, \
             parallel {THREADS} threads (best of {ATTEMPTS}) {parallel_best:?}, \
             speedup {speedup:.2}x"
        );
        // The GIL-release contract's exact witness is
        // `construction_runs_detached` (the hook records the state the
        // construction runs under). This timing probe is the secondary
        // witness: the released GIL lets the threads overlap, so parallel
        // is not slower than serial. The runner's other tests contend for
        // the cores while both legs run, which compresses the ratio (a
        // GIL-holding regression reads ~1.0x, a released one 1.2-3x) — the
        // probe asserts the direction, not a calibrated factor.
        assert!(
            speedup > 1.0,
            "concurrent construction must not be slower than the serial wall \
             time while the GIL is released across the parse+resolve: serial \
             {serial_best:?}, parallel {parallel_best:?}, speedup {speedup:.2}x"
        );
    }

    /// The entry's acceptance test: each of the four construction entry
    /// points runs through `construct_detached`, whose hook records the
    /// GIL state it observes inside the construction; 0 is released. No
    /// threads, no timing.
    #[test]
    fn construction_runs_detached() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (hpxml, schedule, weather, defaults) = fixture_paths();
        let hpxml = hpxml.to_string_lossy().into_owned();
        let schedule = schedule.to_string_lossy().into_owned();
        let weather = weather.to_string_lossy().into_owned();
        let defaults = defaults.to_string_lossy().into_owned();

        Python::attach(|py| {
            let recorded = || CONSTRUCT_GIL_STATE.load(Ordering::SeqCst);
            let (hp, sc, we, de) = (
                PyString::new(py, &hpxml),
                PyString::new(py, &schedule),
                PyString::new(py, &weather),
                PyString::new(py, &defaults),
            );

            CONSTRUCT_GIL_STATE.store(-1, Ordering::SeqCst);
            let mut dwelling = PyDwelling::from_hpxml(
                &py.get_type::<PyDwelling>(),
                hp.as_any(),
                Some(sc.as_any()),
                we.as_any(),
                sim_config_1h_900s(),
                1,
                Some(de.as_any()),
                None,
                None,
            )
            .expect("the cz4a fixture must construct");
            assert_eq!(
                recorded(),
                0,
                "Dwelling.from_hpxml must construct with the GIL released"
            );

            CONSTRUCT_GIL_STATE.store(-1, Ordering::SeqCst);
            dwelling
                .reset_with_seed(py, 7)
                .expect("the reset must rebuild the dwelling");
            assert_eq!(
                recorded(),
                0,
                "reset_with_seed must construct with the GIL released"
            );

            CONSTRUCT_GIL_STATE.store(-1, Ordering::SeqCst);
            let mut blueprint = PyDwellingBlueprint::from_hpxml(
                &py.get_type::<PyDwellingBlueprint>(),
                hp.as_any(),
                Some(sc.as_any()),
                we.as_any(),
                sim_config_1h_900s(),
                1,
                Some(de.as_any()),
                None,
                None,
            )
            .expect("the blueprint must construct");
            assert_eq!(
                recorded(),
                0,
                "DwellingBlueprint.from_hpxml must construct with the GIL released"
            );

            CONSTRUCT_GIL_STATE.store(-1, Ordering::SeqCst);
            let _built = blueprint
                .build(py)
                .expect("the blueprint must build a dwelling");
            assert_eq!(
                recorded(),
                0,
                "DwellingBlueprint.build must build with the GIL released"
            );
        });
    }

    /// The reset the gym environments use restores the warmed-up state
    /// and reseeds without rebuilding: `load_state(snapshot)` then
    /// `reseed(7)` constructs nothing (the test-only build counter is
    /// unchanged; `reset_with_seed` moves it by one, so the counter is
    /// shown to count), the blueprint is the same object throughout, and
    /// the restored dwelling's warm-up health is the snapshot's.
    #[test]
    fn gym_reset_builds_nothing_and_runs_no_warmup() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("tests/fixtures/parity/cz5a_ev_charging");
        Python::attach(|py| {
            let sim_config = crate::py_config::PySimulationConfig::new(
                "2023-01-01T00:00:00-07:00",
                WholeSeconds(86_400),
                WholeSeconds(900),
                None,
                false,
                false,
                hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
                None,
                0,
                None,
                "on",
                None,
                None,
                hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                None,
                crate::py_config::PyWarmupArg::Absent,
                "hour24",
                "typical",
                None,
            )
            .expect("the sim config builds");
            let cls = py.get_type::<PyDwelling>();
            let mut dwelling = PyDwelling::from_hpxml(
                &cls,
                PyString::new(py, fixture.join("building.xml").to_str().expect("utf8")).as_any(),
                Some(
                    PyString::new(py, fixture.join("schedule.csv").to_str().expect("utf8"))
                        .as_any(),
                ),
                PyString::new(py, fixture.join("weather.epw").to_str().expect("utf8")).as_any(),
                sim_config,
                1,
                Some(
                    PyString::new(
                        py,
                        root.join("python/hares/defaults").to_str().expect("utf8"),
                    )
                    .as_any(),
                ),
                None,
                None,
            )
            .expect("the cz5a fixture must construct");
            dwelling.initialize().expect("initialize");

            let warmup_at_snapshot = {
                let guard = dwelling.acquire().expect("not poisoned");
                guard.health().warmup.clone()
            };
            let snapshot = dwelling
                .save_state(py)
                .expect("the warmed-up snapshot saves")
                .unbind();

            let builds_before = crate::py_dwelling::TEST_BUILD_COUNT.load(Ordering::SeqCst);
            let blueprint_before = Arc::clone(&dwelling.blueprint);

            // The reset sequence the environments use.
            use pyo3::prelude::PyBytesMethods;
            let snapshot_bytes: Vec<u8> = snapshot.bind(py).as_bytes().to_vec();
            dwelling
                .load_state(&snapshot_bytes)
                .expect("the snapshot restores");
            dwelling.reseed(7).expect("the reseed succeeds");

            let blueprint_after = Arc::clone(&dwelling.blueprint);
            assert!(
                Arc::ptr_eq(&blueprint_before, &blueprint_after),
                "the reset path must not rebuild: the blueprint is the same object"
            );
            assert_eq!(
                crate::py_dwelling::TEST_BUILD_COUNT.load(Ordering::SeqCst),
                builds_before,
                "the reset path must construct nothing"
            );
            let warmup_after = {
                let guard = dwelling.acquire().expect("not poisoned");
                guard.health().warmup.clone()
            };
            assert_eq!(
                warmup_after, warmup_at_snapshot,
                "the reset keeps the warmed-up state: the warm-up health is the snapshot's"
            );

            // The counter is shown to count: the explicit full rebuild
            // constructs once.
            dwelling
                .reset_with_seed(py, 7)
                .expect("the explicit rebuild succeeds");
            assert_eq!(
                crate::py_dwelling::TEST_BUILD_COUNT.load(Ordering::SeqCst),
                builds_before + 1,
                "reset_with_seed is the one full re-construction"
            );
        });
    }

    /// The reset reuses the blueprint's shared parses: a CSV-schedule
    /// dwelling takes `build_with_seed`'s in-place path, so the
    /// `Arc<DwellingBlueprint>` is the same before and after
    /// `reset_with_seed`, and the reset run's frame equals a fresh build
    /// with that seed, so no input was re-parsed. A generated diversified
    /// schedule re-freezes the blueprint under the seed's key
    /// (`reset_with_seed_reports_the_blueprint_it_built_from`).
    #[test]
    fn reset_with_seed_reuses_blueprint() {
        let _test_count_exclusive = crate::py_dwelling::TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (hpxml, schedule, weather, defaults) = fixture_paths();
        Python::attach(|py| {
            let (hpxml_bound, schedule_bound, weather_bound, defaults_bound) = (
                PyString::new(py, hpxml.to_str().expect("utf8")),
                PyString::new(py, schedule.to_str().expect("utf8")),
                PyString::new(py, weather.to_str().expect("utf8")),
                PyString::new(py, defaults.to_str().expect("utf8")),
            );
            let (hpxml, schedule, weather, defaults) = (
                hpxml_bound.as_any(),
                schedule_bound.as_any(),
                weather_bound.as_any(),
                defaults_bound.as_any(),
            );
            let cls = py.get_type::<PyDwelling>();
            let mut dwelling = PyDwelling::from_hpxml(
                &cls,
                hpxml,
                Some(schedule),
                weather,
                sim_config_1h_900s(),
                1,
                Some(defaults),
                None,
                None,
            )
            .expect("the cz4a fixture must construct");
            let blueprint_before = Arc::clone(&dwelling.blueprint);
            dwelling
                .reset_with_seed(py, 7)
                .expect("the reset must rebuild the dwelling");
            let blueprint_after = Arc::clone(&dwelling.blueprint);
            assert!(
                Arc::ptr_eq(&blueprint_before, &blueprint_after),
                "reset_with_seed must reuse the blueprint, not re-parse"
            );

            // The reset run's frame equals a fresh build with that seed.
            let mut fresh = PyDwelling::from_hpxml(
                &cls,
                hpxml,
                Some(schedule),
                weather,
                sim_config_1h_900s(),
                1,
                Some(defaults),
                None,
                None,
            )
            .expect("the cz4a fixture must construct");
            fresh.reset_with_seed(py, 7).expect("the fresh reset");
            let reset_run = dwelling.simulate(py).expect("the reset dwelling runs");
            let fresh_run = fresh.simulate(py).expect("the fresh dwelling runs");
            let reset_batches: Vec<RecordBatch> = reset_run
                .inner
                .reports
                .iter()
                .flat_map(|report| report.batches().to_vec())
                .collect();
            let fresh_batches: Vec<RecordBatch> = fresh_run
                .inner
                .reports
                .iter()
                .flat_map(|report| report.batches().to_vec())
                .collect();
            assert_eq!(reset_batches.len(), fresh_batches.len(), "batch count");
            for (x, y) in reset_batches.iter().zip(fresh_batches.iter()) {
                assert_eq!(x.num_rows(), y.num_rows(), "row count");
                for (cx, cy) in x.columns().iter().zip(y.columns()) {
                    let (xa, ya) = (
                        cx.as_any().downcast_ref::<Float64Array>(),
                        cy.as_any().downcast_ref::<Float64Array>(),
                    );
                    if let (Some(xa), Some(ya)) = (xa, ya) {
                        for row in 0..x.num_rows() {
                            assert_eq!(
                                xa.value(row).to_bits(),
                                ya.value(row).to_bits(),
                                "the reset run's frame must equal a fresh build's"
                            );
                        }
                    }
                }
            }
        });
    }
}

/// The blueprint the dwelling reports is the one it currently runs:
/// `reset_with_seed` re-freezes a generated diversified schedule under the
/// new seed (the schedule is a build-time draw), so the wrapper's
/// blueprint must move with the build rather than keep the construction
/// seed's. The reset's shared parses (HPXML, weather, defaults) stay the
/// same handles; the generated schedule is the re-derived one.
#[cfg(test)]
mod blueprint_readback_tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use pyo3::Python;
    use pyo3::types::PyString;

    use crate::py_config::{PySimulationConfig, WholeSeconds};
    use crate::py_dwelling::{PyDwelling, TEST_COUNT_EXCLUSIVITY};

    fn fixture_paths() -> (PathBuf, PathBuf, PathBuf) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
        (
            fixture.join("building.xml"),
            fixture.join("weather.epw"),
            root.join("python/hares/defaults"),
        )
    }

    fn sim_config_1h_900s() -> PySimulationConfig {
        PySimulationConfig::new(
            "2023-01-01T00:00:00-07:00",
            WholeSeconds(3600),
            WholeSeconds(900),
            None,
            false,
            false,
            hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
            None,
            0,
            None,
            "on",
            None,
            None,
            hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            None,
            crate::py_config::PyWarmupArg::Absent,
            "hour24",
            "typical",
            None,
        )
        .expect("the sim config builds")
    }

    #[test]
    fn reset_with_seed_reports_the_blueprint_it_built_from() {
        let _test_count_exclusive = TEST_COUNT_EXCLUSIVITY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (hpxml, weather, defaults) = fixture_paths();
        Python::attach(|py| {
            let cls = py.get_type::<PyDwelling>();
            let mut dwelling = PyDwelling::from_hpxml(
                &cls,
                PyString::new(py, hpxml.to_str().expect("utf8")).as_any(),
                // No schedule source: the generated, diversified family.
                None,
                PyString::new(py, weather.to_str().expect("utf8")).as_any(),
                sim_config_1h_900s(),
                1,
                Some(PyString::new(py, defaults.to_str().expect("utf8")).as_any()),
                None,
                None,
            )
            .expect("the generated-schedule fixture must construct");
            let construction_schedule = dwelling.blueprint.inputs().schedule.schedule();
            dwelling
                .reset_with_seed(py, 7)
                .expect("the seed reset must rebuild the dwelling");
            let built = {
                let guard = dwelling
                    .acquire()
                    .expect("the rebuilt dwelling is not poisoned");
                Arc::clone(guard.blueprint())
            };
            let built_schedule = built.inputs().schedule.schedule();
            assert_ne!(
                construction_schedule.columns, built_schedule.columns,
                "the seed swap must re-derive the schedule for the accessor's \
                 equality below to mean anything"
            );
            assert!(
                Arc::ptr_eq(&dwelling.blueprint, &built),
                "the blueprint accessor must report the blueprint the current \
                 dwelling was built from, not the construction seed's"
            );
            let exposed_schedule = dwelling.blueprint.inputs().schedule.schedule();
            assert_eq!(
                exposed_schedule.columns, built_schedule.columns,
                "the exposed blueprint's generated schedule must be the one \
                 the dwelling runs"
            );
        });
    }

    /// The observer snapshot's equipment entry carries `end_use` in the
    /// string vocabulary every other read-back uses (`EndUse::as_str`), the
    /// one a consumer matches against the equipment and end-use names the
    /// reports and the scene tables carry. Rust's `Debug` form
    /// (`EndUse("hvac_heating")`) is a developer rendering of the newtype,
    /// not an end-use name, and no consumer of the snapshot can read it.
    #[cfg(feature = "observe")]
    #[test]
    fn the_observer_snapshot_renders_end_use_as_its_read_back_string() {
        use crate::py_dwelling::snapshot_to_py;
        use chrono::FixedOffset;
        use chrono::TimeZone;
        use hares_core::observer::EquipmentObservation;
        use hares_core::observer::EquipmentPhaseCapture;
        use hares_core::observer::PhaseSnapshots;
        use hares_core::observer::PortsCapture;
        use hares_core::observer::StepSnapshot;
        use hares_types::EndUse;
        use pyo3::types::PyAnyMethods;
        use pyo3::types::PyDict;
        use pyo3::types::PyDictMethods;
        use pyo3::types::PyList;
        use pyo3::types::PyListMethods;

        let snapshot = StepSnapshot {
            step_index: 0,
            timestamp: FixedOffset::east_opt(0)
                .expect("UTC is a valid offset")
                .with_ymd_and_hms(2020, 1, 1, 0, 0, 0)
                .single()
                .expect("the timestamp is unambiguous"),
            phases: PhaseSnapshots {
                post_thermal_equipment: Some(EquipmentPhaseCapture {
                    equipment: vec![EquipmentObservation {
                        name: "Heater".to_string(),
                        equipment_type: "Gas Furnace".to_string(),
                        end_use: EndUse::HVAC_HEATING,
                        ..EquipmentObservation::default()
                    }],
                    ports: PortsCapture::default(),
                }),
                ..PhaseSnapshots::default()
            },
            actor_skips: 0,
            actor_calls: 0,
            actor_error_count: 0,
            moisture_invariant: None,
            time_sin: 0.0,
            time_cos: 0.0,
            actor_telemetry_keys: Vec::new(),
        };

        Python::attach(|py| {
            let rendered = snapshot_to_py(py, &snapshot).expect("the snapshot renders");
            let dict = rendered
                .bind(py)
                .cast::<PyDict>()
                .expect("the snapshot is a dict");
            let phase = dict
                .get_item("post_thermal_equipment")
                .expect("get_item must not raise")
                .expect("the thermal phase is present")
                .cast_into::<PyDict>()
                .expect("the phase is a dict");
            let entries = phase
                .get_item("equipment")
                .expect("get_item must not raise")
                .expect("the phase carries its equipment")
                .cast_into::<PyList>()
                .expect("the equipment entries are a list");
            let end_use: String = entries
                .get_item(0)
                .expect("the observation is listed")
                .cast_into::<PyDict>()
                .expect("an entry is a dict")
                .get_item("end_use")
                .expect("get_item must not raise")
                .expect("the entry carries end_use")
                .extract()
                .expect("end_use is a str");
            assert_eq!(
                end_use,
                EndUse::HVAC_HEATING.as_str(),
                "the snapshot's end_use is the read-back string, not the Debug form"
            );
        });
    }
}
