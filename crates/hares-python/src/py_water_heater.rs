//! Python bindings for typed water heater equipment configuration.

use hares_equipment::EquipmentConfig;
use hares_equipment::water_heater::wh_config::{
    ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig, HeatPumpWaterHeaterConfig,
    IndirectTankConfig, TanklessWaterHeaterConfig,
};
use hares_io::EquipmentSpec;
use hares_types::FuelType;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::{Map, json};

use crate::utils::{insert_opt, make_spec};

// ---------------------------------------------------------------------------
// GasWaterHeater
// ---------------------------------------------------------------------------

/// Gas-fired storage water heater. Supports autosizing when `autosize=True`.
#[pyclass(name = "GasWaterHeater", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasWaterHeater {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub tank_volume_m3: Option<f64>,
    #[pyo3(get)]
    pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)]
    pub heating_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasWaterHeater {
    #[new]
    #[pyo3(signature = (name, autosize=true, tank_volume_m3=None, uniform_energy_factor=None, heating_capacity_w=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>,
        heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && (tank_volume_m3.is_none() || heating_capacity_w.is_none()) {
            return Err(PyValueError::new_err(
                "GasWaterHeater: both tank_volume_m3 and heating_capacity_w are required when autosize=False",
            ));
        }
        if let Some(v) = tank_volume_m3 {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "GasWaterHeater: tank_volume_m3 must be positive",
                ));
            }
        }
        Ok(Self {
            name,
            autosize,
            tank_volume_m3,
            uniform_energy_factor,
            heating_capacity_w,
            setpoint_c,
            zone_id,
            avg_water_draw_l_per_day,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "GasWaterHeater(name={:?}, autosize={})",
            self.name, self.autosize
        )
    }
}

pub fn gas_wh_spec_from_py(wh: &PyGasWaterHeater) -> PyResult<EquipmentSpec> {
    let mut params = Map::new();

    params.insert("fuel_type".into(), json!("natural gas"));
    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", wh.tank_volume_m3);
    insert_opt(
        &mut params,
        "uniform_energy_factor",
        wh.uniform_energy_factor,
    );
    insert_opt(&mut params, "heating_capacity_w", wh.heating_capacity_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    insert_opt(&mut params, "zone_id", wh.zone_id);
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if wh.autosize {
        if let Some(v) = wh.oversizing_factor {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        }
    }

    let typed_config = if wh.autosize {
        None
    } else {
        let volume = Some(wh.tank_volume_m3.unwrap_or(0.19));
        let capacity = Some(wh.heating_capacity_w.unwrap_or(4500.0));
        let cfg = GasWaterHeaterConfig {
            fan_power_w: None,
            fuel_type: FuelType::Gas,
            tank_volume_m3: volume,
            uniform_energy_factor: wh.uniform_energy_factor,
            heating_capacity_w: capacity,
            setpoint_c: wh.setpoint_c,
            zone_id: wh.zone_id,
            avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
            equipment_id: None,
            loop_id: None,
            tank_height_m: None,
            energy_factor: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        };
        Some(
            EquipmentConfig::from_typed(wh.name.clone(), "Gas Water Heater".to_string(), cfg)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        )
    };

    Ok(make_spec(
        "Gas Water Heater",
        &wh.name,
        FuelType::Gas,
        params,
        typed_config,
    ))
}

// ---------------------------------------------------------------------------
// ElectricResistanceWH
// ---------------------------------------------------------------------------

/// Electric resistance storage water heater. Supports autosizing when `autosize=True`.
#[pyclass(name = "ElectricResistanceWH", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricResistanceWH {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub tank_volume_m3: Option<f64>,
    #[pyo3(get)]
    pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)]
    pub heating_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricResistanceWH {
    #[new]
    #[pyo3(signature = (name, autosize=true, tank_volume_m3=None, uniform_energy_factor=None, heating_capacity_w=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>,
        heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && (tank_volume_m3.is_none() || heating_capacity_w.is_none()) {
            return Err(PyValueError::new_err(
                "ElectricResistanceWH: both tank_volume_m3 and heating_capacity_w are required when autosize=False",
            ));
        }
        if let Some(v) = tank_volume_m3 {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "ElectricResistanceWH: tank_volume_m3 must be positive",
                ));
            }
        }
        Ok(Self {
            name,
            autosize,
            tank_volume_m3,
            uniform_energy_factor,
            heating_capacity_w,
            setpoint_c,
            zone_id,
            avg_water_draw_l_per_day,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "ElectricResistanceWH(name={:?}, autosize={})",
            self.name, self.autosize
        )
    }
}

pub fn elec_res_wh_spec_from_py(wh: &PyElectricResistanceWH) -> PyResult<EquipmentSpec> {
    let mut params = Map::new();

    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", wh.tank_volume_m3);
    insert_opt(
        &mut params,
        "uniform_energy_factor",
        wh.uniform_energy_factor,
    );
    insert_opt(&mut params, "heating_capacity_w", wh.heating_capacity_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    insert_opt(&mut params, "zone_id", wh.zone_id);
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if wh.autosize {
        if let Some(v) = wh.oversizing_factor {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        }
    }

    let typed_config = if wh.autosize {
        None
    } else {
        let volume = Some(wh.tank_volume_m3.unwrap_or(0.19));
        let capacity = Some(wh.heating_capacity_w.unwrap_or(4500.0));
        let cfg = ElectricResistanceWaterHeaterConfig {
            tank_volume_m3: volume,
            uniform_energy_factor: wh.uniform_energy_factor,
            heating_capacity_w: capacity,
            setpoint_c: wh.setpoint_c,
            zone_id: wh.zone_id,
            avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
            equipment_id: None,
            loop_id: None,
            tank_height_m: None,
            energy_factor: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        Some(
            EquipmentConfig::from_typed(
                wh.name.clone(),
                "Electric Resistance Water Heater".to_string(),
                cfg,
            )
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        )
    };

    Ok(make_spec(
        "Electric Resistance Water Heater",
        &wh.name,
        FuelType::Electric,
        params,
        typed_config,
    ))
}

// ---------------------------------------------------------------------------
// HeatPumpWH
// ---------------------------------------------------------------------------

/// Heat pump water heater with electric backup. Supports autosizing when `autosize=True`.
#[pyclass(name = "HeatPumpWH", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyHeatPumpWH {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub tank_volume_m3: Option<f64>,
    #[pyo3(get)]
    pub cop: Option<f64>,
    #[pyo3(get)]
    pub backup_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub compressor_power_w: Option<f64>,
    #[pyo3(get)]
    pub setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyHeatPumpWH {
    #[new]
    #[pyo3(signature = (name, autosize=true, tank_volume_m3=None, cop=None, backup_capacity_w=None, compressor_power_w=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        tank_volume_m3: Option<f64>,
        cop: Option<f64>,
        backup_capacity_w: Option<f64>,
        compressor_power_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && (tank_volume_m3.is_none() || backup_capacity_w.is_none()) {
            return Err(PyValueError::new_err(
                "HeatPumpWH: both tank_volume_m3 and backup_capacity_w are required when autosize=False",
            ));
        }
        if let Some(v) = tank_volume_m3 {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "HeatPumpWH: tank_volume_m3 must be positive",
                ));
            }
        }
        if let Some(v) = cop {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err("HeatPumpWH: cop must be positive"));
            }
        }
        Ok(Self {
            name,
            autosize,
            tank_volume_m3,
            cop,
            backup_capacity_w,
            compressor_power_w,
            setpoint_c,
            zone_id,
            avg_water_draw_l_per_day,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "HeatPumpWH(name={:?}, autosize={})",
            self.name, self.autosize
        )
    }
}

pub fn hpwh_spec_from_py(wh: &PyHeatPumpWH) -> PyResult<EquipmentSpec> {
    let mut params = Map::new();

    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", wh.tank_volume_m3);
    insert_opt(&mut params, "cop", wh.cop);
    insert_opt(&mut params, "backup_element_power_w", wh.backup_capacity_w);
    insert_opt(&mut params, "compressor_power_w", wh.compressor_power_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    insert_opt(&mut params, "zone_id", wh.zone_id);
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if wh.autosize {
        if let Some(v) = wh.oversizing_factor {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        }
    }

    let typed_config = if wh.autosize {
        None
    } else {
        let volume = Some(wh.tank_volume_m3.unwrap_or(0.19));
        let backup = Some(wh.backup_capacity_w.unwrap_or(4500.0));
        let cop = Some(wh.cop.unwrap_or(3.5));
        let cfg = HeatPumpWaterHeaterConfig {
            tank_volume_m3: volume,
            cop,
            backup_element_power_w: backup,
            compressor_power_w: wh.compressor_power_w,
            setpoint_c: wh.setpoint_c,
            zone_id: wh.zone_id,
            avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
            equipment_id: None,
            loop_id: None,
            tank_height_m: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            draw_flow_rate_kg_s: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        };
        Some(
            EquipmentConfig::from_typed(wh.name.clone(), "Heat Pump Water Heater".to_string(), cfg)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        )
    };

    Ok(make_spec(
        "Heat Pump Water Heater",
        &wh.name,
        FuelType::Electric,
        params,
        typed_config,
    ))
}

// ---------------------------------------------------------------------------
// TanklessWaterHeater
// ---------------------------------------------------------------------------

/// Tankless (on-demand) gas water heater. Supports autosizing when `autosize=True`.
#[pyclass(name = "TanklessWaterHeater", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyTanklessWaterHeater {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub heating_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)]
    pub setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyTanklessWaterHeater {
    #[new]
    #[pyo3(signature = (name, autosize=true, heating_capacity_w=None, uniform_energy_factor=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        heating_capacity_w: Option<f64>,
        uniform_energy_factor: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && heating_capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "TanklessWaterHeater: heating_capacity_w is required when autosize=False",
            ));
        }
        if let Some(v) = uniform_energy_factor {
            if v <= 0.0 || v > 1.5 {
                return Err(PyValueError::new_err(
                    "TanklessWaterHeater: uniform_energy_factor must be between 0 and 1.5",
                ));
            }
        }
        Ok(Self {
            name,
            autosize,
            heating_capacity_w,
            uniform_energy_factor,
            setpoint_c,
            zone_id,
            avg_water_draw_l_per_day,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "TanklessWaterHeater(name={:?}, autosize={})",
            self.name, self.autosize
        )
    }
}

pub fn tankless_wh_spec_from_py(wh: &PyTanklessWaterHeater) -> PyResult<EquipmentSpec> {
    let mut params = Map::new();

    params.insert("fuel_type".into(), json!("natural gas"));
    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(
        &mut params,
        "uniform_energy_factor",
        wh.uniform_energy_factor,
    );
    insert_opt(&mut params, "heating_capacity_w", wh.heating_capacity_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    insert_opt(&mut params, "zone_id", wh.zone_id);
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if wh.autosize {
        if let Some(v) = wh.oversizing_factor {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        }
    }

    let typed_config = if wh.autosize {
        None
    } else {
        let cfg = TanklessWaterHeaterConfig {
            fuel_type: FuelType::Gas,
            uniform_energy_factor: wh.uniform_energy_factor,
            heating_capacity_w: wh.heating_capacity_w,
            setpoint_c: wh.setpoint_c,
            zone_id: wh.zone_id,
            avg_water_draw_l_per_day: wh.avg_water_draw_l_per_day,
            equipment_id: None,
            loop_id: None,
            energy_factor: None,
            parasitic_power_w: None,
            performance_adjustment: None,
            inlet_temp_c: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            zone_type: None,
            min_flow_kg_s: None,
            min_flow_gpm: None,
        };
        Some(
            EquipmentConfig::from_typed(wh.name.clone(), "Tankless Water Heater".to_string(), cfg)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        )
    };

    Ok(make_spec(
        "Tankless Water Heater",
        &wh.name,
        FuelType::Gas,
        params,
        typed_config,
    ))
}

// ---------------------------------------------------------------------------
// IndirectTank
// ---------------------------------------------------------------------------

/// Indirect storage tank heated by a boiler. Supports autosizing when `autosize=True`.
#[pyclass(name = "IndirectTank", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyIndirectTank {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub tank_volume_m3: Option<f64>,
    #[pyo3(get)]
    pub hx_ua_w_per_k: Option<f64>,
    #[pyo3(get)]
    pub setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub boiler_loop_id: Option<u16>,
    #[pyo3(get)]
    pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyIndirectTank {
    #[new]
    #[pyo3(signature = (name, autosize=true, tank_volume_m3=None, hx_ua_w_per_k=None, setpoint_c=None, zone_id=None, boiler_loop_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        tank_volume_m3: Option<f64>,
        hx_ua_w_per_k: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        boiler_loop_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && tank_volume_m3.is_none() {
            return Err(PyValueError::new_err(
                "IndirectTank: tank_volume_m3 is required when autosize=False",
            ));
        }
        if let Some(v) = tank_volume_m3 {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "IndirectTank: tank_volume_m3 must be positive",
                ));
            }
        }
        Ok(Self {
            name,
            autosize,
            tank_volume_m3,
            hx_ua_w_per_k,
            setpoint_c,
            zone_id,
            boiler_loop_id,
            avg_water_draw_l_per_day,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "IndirectTank(name={:?}, autosize={})",
            self.name, self.autosize
        )
    }
}

pub fn indirect_tank_spec_from_py(it: &PyIndirectTank) -> PyResult<EquipmentSpec> {
    let mut params = Map::new();

    if it.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", it.tank_volume_m3);
    insert_opt(&mut params, "hx_ua_w_per_k", it.hx_ua_w_per_k);
    insert_opt(&mut params, "setpoint_c", it.setpoint_c);
    insert_opt(&mut params, "zone_id", it.zone_id);
    insert_opt(&mut params, "boiler_loop_id", it.boiler_loop_id);
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        it.avg_water_draw_l_per_day,
    );
    if it.autosize {
        if let Some(v) = it.oversizing_factor {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        }
    }

    let typed_config = if it.autosize {
        None
    } else {
        let cfg = IndirectTankConfig {
            tank_volume_m3: it.tank_volume_m3,
            hx_ua_w_per_k: it.hx_ua_w_per_k,
            setpoint_c: it.setpoint_c,
            zone_id: it.zone_id,
            boiler_loop_id: it.boiler_loop_id,
            avg_water_draw_l_per_day: it.avg_water_draw_l_per_day,
            equipment_id: None,
            tank_height_m: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        };
        Some(
            EquipmentConfig::from_typed(it.name.clone(), "Indirect Tank".to_string(), cfg)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        )
    };

    Ok(make_spec(
        "Indirect Tank",
        &it.name,
        FuelType::Gas,
        params,
        typed_config,
    ))
}
