//! Python bindings for typed water heater equipment configuration.

use serde_json::{json, Map, Value};
use pyo3::prelude::*;
use pyo3::exceptions::PyValueError;
use hares_types::FuelType;
use hares_io::EquipmentSpec;
use hares_equipment::EquipmentConfig;
use hares_equipment::water_heater::wh_config::{
    GasWaterHeaterConfig, ElectricResistanceWaterHeaterConfig, HeatPumpWaterHeaterConfig,
};

use crate::utils::insert_opt;

// ---------------------------------------------------------------------------
// GasWaterHeater
// ---------------------------------------------------------------------------

#[pyclass(name = "GasWaterHeater", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasWaterHeater {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasWaterHeater {
    #[new]
    #[pyo3(signature = (name, autosize=None, tank_volume_m3=None, uniform_energy_factor=None, heating_capacity_w=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: Option<bool>,
        tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>,
        heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        let autosize = autosize.unwrap_or(tank_volume_m3.is_none() || heating_capacity_w.is_none());
        if !autosize && tank_volume_m3.is_none() && heating_capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "GasWaterHeater: at least one of tank_volume_m3 or heating_capacity_w is required when autosize=False",
            ));
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

pub fn gas_wh_spec_from_py(wh: &PyGasWaterHeater) -> EquipmentSpec {
    let mut params = Map::new();

    params.insert("fuel_type".into(), json!("natural gas"));
    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", wh.tank_volume_m3);
    insert_opt(&mut params, "uniform_energy_factor", wh.uniform_energy_factor);
    insert_opt(&mut params, "heating_capacity_w", wh.heating_capacity_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    if let Some(v) = wh.zone_id {
        params.insert("zone_id".into(), json!(v));
    }
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if let Some(v) = wh.oversizing_factor {
        if wh.autosize {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let volume = wh.tank_volume_m3.or(Some(0.19));
    let capacity = wh.heating_capacity_w.or(Some(0.0));
    let cfg = GasWaterHeaterConfig {
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
    let typed_config = Some(EquipmentConfig::from_typed(
        wh.name.clone(),
        "Gas Water Heater".to_string(),
        cfg,
    ));

    EquipmentSpec {
        name: "Gas Water Heater".to_string(),
        instance_name: Some(wh.name.clone()),
        fuel_type: FuelType::Gas,
        parameters: params,
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

// ---------------------------------------------------------------------------
// ElectricResistanceWH
// ---------------------------------------------------------------------------

#[pyclass(name = "ElectricResistanceWH", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricResistanceWH {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub uniform_energy_factor: Option<f64>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricResistanceWH {
    #[new]
    #[pyo3(signature = (name, autosize=None, tank_volume_m3=None, uniform_energy_factor=None, heating_capacity_w=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: Option<bool>,
        tank_volume_m3: Option<f64>,
        uniform_energy_factor: Option<f64>,
        heating_capacity_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        let autosize = autosize.unwrap_or(tank_volume_m3.is_none() || heating_capacity_w.is_none());
        if !autosize && tank_volume_m3.is_none() && heating_capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ElectricResistanceWH: at least one of tank_volume_m3 or heating_capacity_w is required when autosize=False",
            ));
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

pub fn elec_res_wh_spec_from_py(wh: &PyElectricResistanceWH) -> EquipmentSpec {
    let mut params = Map::new();

    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", wh.tank_volume_m3);
    insert_opt(&mut params, "uniform_energy_factor", wh.uniform_energy_factor);
    insert_opt(&mut params, "heating_capacity_w", wh.heating_capacity_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    if let Some(v) = wh.zone_id {
        params.insert("zone_id".into(), json!(v));
    }
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if let Some(v) = wh.oversizing_factor {
        if wh.autosize {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let volume = wh.tank_volume_m3.or(Some(0.19));
    let capacity = wh.heating_capacity_w.or(Some(0.0));
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
    let typed_config = Some(EquipmentConfig::from_typed(
        wh.name.clone(),
        "Electric Resistance Water Heater".to_string(),
        cfg,
    ));

    EquipmentSpec {
        name: "Electric Resistance Water Heater".to_string(),
        instance_name: Some(wh.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params,
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}

// ---------------------------------------------------------------------------
// HeatPumpWH
// ---------------------------------------------------------------------------

#[pyclass(name = "HeatPumpWH", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyHeatPumpWH {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub tank_volume_m3: Option<f64>,
    #[pyo3(get)] pub cop: Option<f64>,
    #[pyo3(get)] pub backup_element_power_w: Option<f64>,
    #[pyo3(get)] pub compressor_power_w: Option<f64>,
    #[pyo3(get)] pub setpoint_c: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub avg_water_draw_l_per_day: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyHeatPumpWH {
    #[new]
    #[pyo3(signature = (name, autosize=None, tank_volume_m3=None, cop=None, backup_element_power_w=None, compressor_power_w=None, setpoint_c=None, zone_id=None, avg_water_draw_l_per_day=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: Option<bool>,
        tank_volume_m3: Option<f64>,
        cop: Option<f64>,
        backup_element_power_w: Option<f64>,
        compressor_power_w: Option<f64>,
        setpoint_c: Option<f64>,
        zone_id: Option<u16>,
        avg_water_draw_l_per_day: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        let autosize = autosize
            .unwrap_or(tank_volume_m3.is_none() || backup_element_power_w.is_none());
        if !autosize && tank_volume_m3.is_none() && backup_element_power_w.is_none() {
            return Err(PyValueError::new_err(
                "HeatPumpWH: at least one of tank_volume_m3 or backup_element_power_w is required when autosize=False",
            ));
        }
        Ok(Self {
            name,
            autosize,
            tank_volume_m3,
            cop,
            backup_element_power_w,
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

pub fn hpwh_spec_from_py(wh: &PyHeatPumpWH) -> EquipmentSpec {
    let mut params = Map::new();

    if wh.autosize {
        params.insert("autosize_water_heater".into(), json!(true));
    }
    insert_opt(&mut params, "tank_volume_m3", wh.tank_volume_m3);
    insert_opt(&mut params, "cop", wh.cop);
    insert_opt(
        &mut params,
        "backup_element_power_w",
        wh.backup_element_power_w,
    );
    insert_opt(&mut params, "compressor_power_w", wh.compressor_power_w);
    insert_opt(&mut params, "setpoint_c", wh.setpoint_c);
    if let Some(v) = wh.zone_id {
        params.insert("zone_id".into(), json!(v));
    }
    insert_opt(
        &mut params,
        "avg_water_draw_l_per_day",
        wh.avg_water_draw_l_per_day,
    );
    if let Some(v) = wh.oversizing_factor {
        if wh.autosize {
            params.insert("autosize_water_heater_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let volume = wh.tank_volume_m3.or(Some(0.19));
    let backup = wh.backup_element_power_w.or(Some(0.0));
    let cop = wh.cop.or(Some(3.5));
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
    };
    let typed_config = Some(EquipmentConfig::from_typed(
        wh.name.clone(),
        "Heat Pump Water Heater".to_string(),
        cfg,
    ));

    EquipmentSpec {
        name: "Heat Pump Water Heater".to_string(),
        instance_name: Some(wh.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params,
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}
