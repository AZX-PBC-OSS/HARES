//! Python bindings for typed HVAC heating/cooling equipment configuration.

use serde_json::{json, Map};
use pyo3::prelude::*;
use pyo3::exceptions::PyValueError;
use hares_types::FuelType;
use hares_io::EquipmentSpec;
use hares_equipment::EquipmentConfig;
use hares_equipment::hvac::heating_config::{
    GasFurnaceConfig, HvacSetpointConfig, ElectricBaseboardConfig, IdealHvacConfig, DuctConfig,
};
use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
use hares_equipment::hvac::heat_pump_config::{
    HeatPumpHeaterConfig, HeatPumpCoolerConfig, HeatPumpCommonConfig,
};

use crate::utils::insert_opt;

const BTU_PER_HR_PER_W: f64 = 3.412_141_633;

fn setpoint_config(
    heating_setpoint_c: Option<f64>,
    cooling_setpoint_c: Option<f64>,
) -> HvacSetpointConfig {
    HvacSetpointConfig {
        heating_setpoint_c,
        cooling_setpoint_c,
        ..HvacSetpointConfig::default()
    }
}

// ---------------------------------------------------------------------------
// GasFurnace
// ---------------------------------------------------------------------------

#[pyclass(name = "GasFurnace", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasFurnace {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub afue: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub number_of_speeds: Option<u8>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasFurnace {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, afue=None, zone_id=None, fan_power_w=None, number_of_speeds=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        afue: Option<f64>,
        zone_id: Option<u16>,
        fan_power_w: Option<f64>,
        number_of_speeds: Option<u8>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "GasFurnace: capacity_w is required when autosize=False",
            ));
        }
        Ok(Self {
            name,
            autosize,
            capacity_w,
            afue,
            zone_id,
            fan_power_w,
            number_of_speeds,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "GasFurnace(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

pub fn gas_furnace_spec_from_py(gf: &PyGasFurnace) -> EquipmentSpec {
    let mut params = Map::new();

    if gf.autosize {
        params.insert("autosize_heating".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", gf.capacity_w);
    insert_opt(&mut params, "afue", gf.afue);
    insert_opt(&mut params, "zone_id", gf.zone_id);
    insert_opt(&mut params, "fan_power_w", gf.fan_power_w);
    insert_opt(&mut params, "number_of_speeds", gf.number_of_speeds);
    insert_opt(&mut params, "heating_setpoint_c", gf.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", gf.cooling_setpoint_c);
    if let Some(v) = gf.oversizing_factor {
        if gf.autosize {
            params.insert("autosize_heating_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let typed_config = if gf.autosize {
        None
    } else {
        let cfg = GasFurnaceConfig {
            capacity_w: gf.capacity_w.unwrap(),
            afue: gf.afue.unwrap_or(0.80),
            zone_id: gf.zone_id,
            fan_power_w: gf.fan_power_w,
            number_of_speeds: gf.number_of_speeds.unwrap_or(1),
            setpoint: setpoint_config(gf.heating_setpoint_c, gf.cooling_setpoint_c),
            ..GasFurnaceConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            gf.name.clone(),
            "Gas Furnace".to_string(),
            cfg,
        ))
    };

    EquipmentSpec {
        name: "Gas Furnace".to_string(),
        instance_name: Some(gf.name.clone()),
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
// AirConditioner
// ---------------------------------------------------------------------------

#[pyclass(name = "AirConditioner", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyAirConditioner {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub eir: Option<f64>,
    #[pyo3(get)] pub seer: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub shr: Option<f64>,
    #[pyo3(get)] pub number_of_speeds: Option<u8>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyAirConditioner {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, eir=None, seer=None, zone_id=None, shr=None, number_of_speeds=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        eir: Option<f64>,
        seer: Option<f64>,
        zone_id: Option<u16>,
        shr: Option<f64>,
        number_of_speeds: Option<u8>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "AirConditioner: capacity_w is required when autosize=False",
            ));
        }
        Ok(Self {
            name,
            autosize,
            capacity_w,
            eir,
            seer,
            zone_id,
            shr,
            number_of_speeds,
            fan_power_w,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "AirConditioner(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

fn resolve_eir(eir: Option<f64>, seer: Option<f64>, default_seer: f64) -> f64 {
    eir.unwrap_or_else(|| {
        let effective_seer = seer.unwrap_or(default_seer);
        BTU_PER_HR_PER_W / effective_seer.max(1e-6)
    })
}

pub fn ac_spec_from_py(ac: &PyAirConditioner) -> EquipmentSpec {
    let mut params = Map::new();
    let eir = resolve_eir(ac.eir, ac.seer, 14.0);

    if ac.autosize {
        params.insert("autosize_cooling".into(), json!(true));
    }
    insert_opt(&mut params, "cooling_capacity_w", ac.capacity_w);
    params.insert("eir".into(), json!(eir));
    insert_opt(&mut params, "seer", ac.seer);
    insert_opt(&mut params, "zone_id", ac.zone_id);
    insert_opt(&mut params, "shr", ac.shr);
    insert_opt(&mut params, "number_of_speeds", ac.number_of_speeds);
    insert_opt(&mut params, "fan_power_w", ac.fan_power_w);
    insert_opt(&mut params, "heating_setpoint_c", ac.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", ac.cooling_setpoint_c);
    if let Some(v) = ac.oversizing_factor {
        if ac.autosize {
            params.insert("autosize_cooling_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let typed_config = if ac.autosize {
        None
    } else {
        let cfg = CentralAirConditionerConfig {
            capacity_w: ac.capacity_w.unwrap(),
            eir,
            zone_id: ac.zone_id,
            shr: ac.shr,
            number_of_speeds: ac.number_of_speeds.unwrap_or(1),
            fan_power_w: ac.fan_power_w,
            setpoint: setpoint_config(ac.heating_setpoint_c, ac.cooling_setpoint_c),
            equipment_id: None,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w_per_cfm: None,
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            fraction_load_served: None,
            crankcase_heater_kw: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        Some(EquipmentConfig::from_typed(
            ac.name.clone(),
            "Air Conditioner".to_string(),
            cfg,
        ))
    };

    EquipmentSpec {
        name: "Air Conditioner".to_string(),
        instance_name: Some(ac.name.clone()),
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
// ASHPHeater
// ---------------------------------------------------------------------------

#[pyclass(name = "ASHPHeater", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyASHPHeater {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub hspf: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub is_mini_split: Option<bool>,
    #[pyo3(get)] pub backup_capacity_w: Option<f64>,
    #[pyo3(get)] pub backup_fuel: Option<String>,
    /// Pre-parsed fuel type, validated during construction.
    pub(crate) backup_fuel_parsed: Option<FuelType>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyASHPHeater {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, hspf=None, zone_id=None, is_mini_split=None, backup_capacity_w=None, backup_fuel=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        hspf: Option<f64>,
        zone_id: Option<u16>,
        is_mini_split: Option<bool>,
        backup_capacity_w: Option<f64>,
        backup_fuel: Option<String>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ASHPHeater: capacity_w is required when autosize=False",
            ));
        }
        let backup_fuel_parsed = parse_fuel_type(backup_fuel.as_deref())?;
        Ok(Self {
            name,
            autosize,
            capacity_w,
            hspf,
            zone_id,
            is_mini_split,
            backup_capacity_w,
            backup_fuel,
            backup_fuel_parsed,
            fan_power_w,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "ASHPHeater(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

fn parse_fuel_type(s: Option<&str>) -> PyResult<Option<FuelType>> {
    match s.map(|s| s.to_lowercase()).as_deref() {
        None => Ok(None),
        Some("electric") => Ok(Some(FuelType::Electric)),
        Some("gas") | Some("natural gas") => Ok(Some(FuelType::Gas)),
        Some("propane") => Ok(Some(FuelType::Propane)),
        Some("oil") | Some("fuel oil") => Ok(Some(FuelType::Oil)),
        Some("wood") => Ok(Some(FuelType::Wood)),
        Some("coal") => Ok(Some(FuelType::Coal)),
        Some("woodpellet") | Some("wood_pellet") => Ok(Some(FuelType::WoodPellet)),
        Some("none") | Some("nofuel") => Ok(Some(FuelType::None)),
        Some(invalid) => Err(PyValueError::new_err(format!(
            "ASHPHeater: invalid backup_fuel '{invalid}'. \
             Valid: Electric, Gas, Natural Gas, Propane, Oil, Fuel Oil, Wood, Coal, WoodPellet, None"
        ))),
    }
}

pub fn ashp_heater_spec_from_py(hp: &PyASHPHeater) -> EquipmentSpec {
    let mut params = Map::new();
    let hspf = hp.hspf.unwrap_or(8.2);
    let heating_eir = BTU_PER_HR_PER_W / hspf.max(1e-6);

    if hp.autosize {
        params.insert("autosize_heating".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", hp.capacity_w);
    params.insert("heating_eir".into(), json!(heating_eir));
    if hp.hspf.is_some() {
        params.insert("hspf".into(), json!(hspf));
    }
    insert_opt(&mut params, "is_mini_split", hp.is_mini_split);
    insert_opt(&mut params, "zone_id", hp.zone_id);
    insert_opt(&mut params, "backup_capacity_w", hp.backup_capacity_w);
    if let Some(ref fuel) = hp.backup_fuel {
        params.insert("backup_fuel".into(), json!(fuel));
    }
    insert_opt(&mut params, "fan_power_w", hp.fan_power_w);
    insert_opt(&mut params, "heating_setpoint_c", hp.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", hp.cooling_setpoint_c);
    if let Some(v) = hp.oversizing_factor {
        if hp.autosize {
            params.insert("autosize_heating_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let typed_config = if hp.autosize {
        None
    } else {
        let common = HeatPumpCommonConfig {
            heating_capacity_w: hp.capacity_w,
            heating_eir: Some(heating_eir),
            zone_id: hp.zone_id,
            is_mini_split: hp.is_mini_split.unwrap_or(false),
            backup_capacity_w: hp.backup_capacity_w,
            backup_fuel: hp.backup_fuel_parsed,
            fan_power_w: hp.fan_power_w,
            setpoint: setpoint_config(hp.heating_setpoint_c, hp.cooling_setpoint_c),
            ..HeatPumpCommonConfig::default()
        };
        let cfg = HeatPumpHeaterConfig {
            common,
            ..HeatPumpHeaterConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            hp.name.clone(),
            "ASHP Heater".to_string(),
            cfg,
        ))
    };

    EquipmentSpec {
        name: "ASHP Heater".to_string(),
        instance_name: Some(hp.name.clone()),
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
// ASHPCooler
// ---------------------------------------------------------------------------

#[pyclass(name = "ASHPCooler", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyASHPCooler {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub seer: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub is_mini_split: Option<bool>,
    #[pyo3(get)] pub shr: Option<f64>,
    #[pyo3(get)] pub fan_power_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyASHPCooler {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, seer=None, zone_id=None, is_mini_split=None, shr=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        seer: Option<f64>,
        zone_id: Option<u16>,
        is_mini_split: Option<bool>,
        shr: Option<f64>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ASHPCooler: capacity_w is required when autosize=False",
            ));
        }
        Ok(Self {
            name,
            autosize,
            capacity_w,
            seer,
            zone_id,
            is_mini_split,
            shr,
            fan_power_w,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "ASHPCooler(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

pub fn ashp_cooler_spec_from_py(hp: &PyASHPCooler) -> EquipmentSpec {
    let mut params = Map::new();
    let seer = hp.seer.unwrap_or(14.0);
    let cooling_eir = BTU_PER_HR_PER_W / seer.max(1e-6);

    if hp.autosize {
        params.insert("autosize_cooling".into(), json!(true));
    }
    insert_opt(&mut params, "cooling_capacity_w", hp.capacity_w);
    params.insert("cooling_eir".into(), json!(cooling_eir));
    if hp.seer.is_some() {
        params.insert("seer".into(), json!(seer));
    }
    insert_opt(&mut params, "is_mini_split", hp.is_mini_split);
    insert_opt(&mut params, "zone_id", hp.zone_id);
    insert_opt(&mut params, "shr", hp.shr);
    insert_opt(&mut params, "fan_power_w", hp.fan_power_w);
    insert_opt(&mut params, "heating_setpoint_c", hp.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", hp.cooling_setpoint_c);
    if let Some(v) = hp.oversizing_factor {
        if hp.autosize {
            params.insert("autosize_cooling_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let typed_config = if hp.autosize {
        None
    } else {
        let common = HeatPumpCommonConfig {
            cooling_capacity_w: hp.capacity_w,
            cooling_eir: Some(cooling_eir),
            zone_id: hp.zone_id,
            is_mini_split: hp.is_mini_split.unwrap_or(false),
            shr: hp.shr,
            fan_power_w: hp.fan_power_w,
            setpoint: setpoint_config(hp.heating_setpoint_c, hp.cooling_setpoint_c),
            ..HeatPumpCommonConfig::default()
        };
        let cfg = HeatPumpCoolerConfig {
            common,
            ..HeatPumpCoolerConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            hp.name.clone(),
            "ASHP Cooler".to_string(),
            cfg,
        ))
    };

    EquipmentSpec {
        name: "ASHP Cooler".to_string(),
        instance_name: Some(hp.name.clone()),
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
// ElectricBaseboard
// ---------------------------------------------------------------------------

#[pyclass(name = "ElectricBaseboard", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricBaseboard {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub autosize: bool,
    #[pyo3(get)] pub capacity_w: Option<f64>,
    #[pyo3(get)] pub eir: Option<f64>,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricBaseboard {
    #[new]
    #[pyo3(signature = (name, autosize=false, capacity_w=None, eir=None, zone_id=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        eir: Option<f64>,
        zone_id: Option<u16>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ElectricBaseboard: capacity_w is required when autosize=False",
            ));
        }
        Ok(Self {
            name,
            autosize,
            capacity_w,
            eir,
            zone_id,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "ElectricBaseboard(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

pub fn baseboard_spec_from_py(bb: &PyElectricBaseboard) -> EquipmentSpec {
    let mut params = Map::new();
    let eir = bb.eir.unwrap_or(1.0);

    if bb.autosize {
        params.insert("autosize_heating".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", bb.capacity_w);
    params.insert("eir".into(), json!(eir));
    insert_opt(&mut params, "zone_id", bb.zone_id);
    insert_opt(&mut params, "heating_setpoint_c", bb.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", bb.cooling_setpoint_c);
    if let Some(v) = bb.oversizing_factor {
        if bb.autosize {
            params.insert("autosize_heating_factor".into(), json!(v));
        } else {
            params.insert("oversizing_factor".into(), json!(v));
        }
    }

    let typed_config = if bb.autosize {
        None
    } else {
        let cfg = ElectricBaseboardConfig {
            capacity_w: bb.capacity_w.unwrap(),
            eir,
            zone_id: bb.zone_id,
            setpoint: setpoint_config(bb.heating_setpoint_c, bb.cooling_setpoint_c),
            ..ElectricBaseboardConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            bb.name.clone(),
            "Electric Baseboard".to_string(),
            cfg,
        ))
    };

    EquipmentSpec {
        name: "Electric Baseboard".to_string(),
        instance_name: Some(bb.name.clone()),
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
// IdealHVAC
// ---------------------------------------------------------------------------

#[pyclass(name = "IdealHVAC", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyIdealHVAC {
    #[pyo3(get)] pub name: String,
    #[pyo3(get)] pub zone_id: Option<u16>,
    #[pyo3(get)] pub heating_capacity_w: Option<f64>,
    #[pyo3(get)] pub cooling_capacity_w: Option<f64>,
    #[pyo3(get)] pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)] pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)] pub deadband_c: Option<f64>,
}

#[pymethods]
impl PyIdealHVAC {
    #[new]
    #[pyo3(signature = (name, zone_id=None, heating_capacity_w=None, cooling_capacity_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, deadband_c=None))]
    fn new(
        name: String,
        zone_id: Option<u16>,
        heating_capacity_w: Option<f64>,
        cooling_capacity_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> PyResult<Self> {
        Ok(Self {
            name,
            zone_id,
            heating_capacity_w,
            cooling_capacity_w,
            heating_setpoint_c,
            cooling_setpoint_c,
            deadband_c,
        })
    }

    fn __repr__(&self) -> String {
        format!("IdealHVAC(name={:?})", self.name)
    }
}

pub fn ideal_hvac_spec_from_py(ih: &PyIdealHVAC) -> EquipmentSpec {
    let mut params = Map::new();

    insert_opt(&mut params, "heating_capacity_w", ih.heating_capacity_w);
    insert_opt(&mut params, "cooling_capacity_w", ih.cooling_capacity_w);
    insert_opt(&mut params, "zone_id", ih.zone_id);
    insert_opt(&mut params, "heating_setpoint_c", ih.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", ih.cooling_setpoint_c);
    insert_opt(&mut params, "deadband_c", ih.deadband_c);

    let cfg = IdealHvacConfig {
        heating_capacity_w: ih.heating_capacity_w,
        cooling_capacity_w: ih.cooling_capacity_w,
        zone_id: ih.zone_id,
        deadband_c: ih.deadband_c,
        setpoint: setpoint_config(ih.heating_setpoint_c, ih.cooling_setpoint_c),
        ..IdealHvacConfig::default()
    };
    let typed_config = Some(EquipmentConfig::from_typed(
        ih.name.clone(),
        "Ideal HVAC".to_string(),
        cfg,
    ));

    EquipmentSpec {
        name: "Ideal HVAC".to_string(),
        instance_name: Some(ih.name.clone()),
        fuel_type: FuelType::Electric,
        parameters: params,
        zip_params: None,
        typed_config,
        system_id: None,
        related_hvac_idref: None,
        primary_role: None,
    }
}
