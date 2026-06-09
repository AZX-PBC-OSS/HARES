//! Python bindings for typed HVAC heating/cooling equipment configuration.

use hares_equipment::EquipmentConfig;
use hares_equipment::hvac::cooling_config::CentralAirConditionerConfig;
use hares_equipment::hvac::heat_pump_config::{
    HeatPumpCommonConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
};
use hares_equipment::hvac::heating_config::{
    DuctConfig, ElectricBaseboardConfig, ElectricBoilerConfig, ElectricFurnaceConfig,
    GasBoilerConfig, GasFurnaceConfig, HvacSetpointConfig, IdealHvacConfig,
};
use hares_io::EquipmentSpec;
use hares_types::FuelType;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::{Map, json};

use crate::utils::{insert_opt, make_spec};

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

/// Gas furnace heating equipment. Supports autosizing when `autosize=True`.
#[pyclass(name = "GasFurnace", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasFurnace {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub afue: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub fan_power_w: Option<f64>,
    #[pyo3(get)]
    pub number_of_speeds: Option<u8>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasFurnace {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, afue=None, zone_id=None, fan_power_w=None, number_of_speeds=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
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
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "GasFurnace: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = afue {
            if v <= 0.0 || v > 1.0 {
                return Err(PyValueError::new_err(
                    "GasFurnace: afue must be between 0 and 1",
                ));
            }
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
    insert_opt(&mut params, "efficiency_afue", gf.afue);
    insert_opt(&mut params, "zone_id", gf.zone_id);
    insert_opt(&mut params, "fan_power_w", gf.fan_power_w);
    insert_opt(&mut params, "number_of_speeds", gf.number_of_speeds);
    insert_opt(&mut params, "heating_setpoint_c", gf.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", gf.cooling_setpoint_c);
    if gf.autosize {
        if let Some(v) = gf.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
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

    make_spec("Gas Furnace", &gf.name, FuelType::Gas, params, typed_config)
}

// ---------------------------------------------------------------------------
// AirConditioner
// ---------------------------------------------------------------------------

/// Central air conditioner cooling equipment. Supports autosizing when `autosize=True`.
#[pyclass(name = "AirConditioner", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyAirConditioner {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub eir: Option<f64>,
    #[pyo3(get)]
    pub seer: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub shr: Option<f64>,
    #[pyo3(get)]
    pub number_of_speeds: Option<u8>,
    #[pyo3(get)]
    pub fan_power_w: Option<f64>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyAirConditioner {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, eir=None, seer=None, zone_id=None, shr=None, number_of_speeds=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
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
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "AirConditioner: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = seer {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "AirConditioner: seer must be positive",
                ));
            }
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
    insert_opt(&mut params, "efficiency_seer", ac.seer);
    insert_opt(&mut params, "zone_id", ac.zone_id);
    insert_opt(&mut params, "shr", ac.shr);
    insert_opt(&mut params, "number_of_speeds", ac.number_of_speeds);
    insert_opt(&mut params, "fan_power_w", ac.fan_power_w);
    insert_opt(&mut params, "heating_setpoint_c", ac.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", ac.cooling_setpoint_c);
    if ac.autosize {
        if let Some(v) = ac.oversizing_factor {
            params.insert("autosize_cooling_factor".into(), json!(v));
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

    make_spec(
        "Air Conditioner",
        &ac.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}

// ---------------------------------------------------------------------------
// ASHPHeater
// ---------------------------------------------------------------------------

/// Air-source heat pump in heating mode. Supports autosizing when `autosize=True`.
#[pyclass(name = "ASHPHeater", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyASHPHeater {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub hspf: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub is_mini_split: Option<bool>,
    #[pyo3(get)]
    pub backup_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub backup_fuel: Option<String>,
    /// Pre-parsed fuel type, validated during construction.
    pub(crate) backup_fuel_parsed: Option<FuelType>,
    #[pyo3(get)]
    pub fan_power_w: Option<f64>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyASHPHeater {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, hspf=None, zone_id=None, is_mini_split=None, backup_capacity_w=None, backup_fuel=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
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
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "ASHPHeater: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = hspf {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err("ASHPHeater: hspf must be positive"));
            }
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
            "invalid backup_fuel '{invalid}'. \
             Valid: Electric, Gas, Natural Gas, Propane, Oil, Fuel Oil, Wood, Coal, WoodPellet, NoFuel"
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
        params.insert("efficiency_hspf".into(), json!(hspf));
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
    if hp.autosize {
        if let Some(v) = hp.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
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

    make_spec(
        "ASHP Heater",
        &hp.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}

// ---------------------------------------------------------------------------
// ASHPCooler
// ---------------------------------------------------------------------------

/// Air-source heat pump in cooling mode. Supports autosizing when `autosize=True`.
#[pyclass(name = "ASHPCooler", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyASHPCooler {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub seer: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub is_mini_split: Option<bool>,
    #[pyo3(get)]
    pub shr: Option<f64>,
    #[pyo3(get)]
    pub fan_power_w: Option<f64>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyASHPCooler {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, seer=None, zone_id=None, is_mini_split=None, shr=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
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
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "ASHPCooler: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = seer {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err("ASHPCooler: seer must be positive"));
            }
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
        params.insert("efficiency_seer".into(), json!(seer));
    }
    insert_opt(&mut params, "is_mini_split", hp.is_mini_split);
    insert_opt(&mut params, "zone_id", hp.zone_id);
    insert_opt(&mut params, "shr", hp.shr);
    insert_opt(&mut params, "fan_power_w", hp.fan_power_w);
    insert_opt(&mut params, "heating_setpoint_c", hp.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", hp.cooling_setpoint_c);
    if hp.autosize {
        if let Some(v) = hp.oversizing_factor {
            params.insert("autosize_cooling_factor".into(), json!(v));
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

    make_spec(
        "ASHP Cooler",
        &hp.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}

// ---------------------------------------------------------------------------
// ElectricBaseboard
// ---------------------------------------------------------------------------

/// Electric baseboard (resistive) heating equipment. Supports autosizing when `autosize=True`.
#[pyclass(name = "ElectricBaseboard", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricBaseboard {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub eir: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricBaseboard {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, eir=None, zone_id=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
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
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "ElectricBaseboard: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = eir {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "ElectricBaseboard: eir must be positive",
                ));
            }
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
    params.insert("heating_efficiency".into(), json!(eir));
    insert_opt(&mut params, "zone_id", bb.zone_id);
    insert_opt(&mut params, "heating_setpoint_c", bb.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", bb.cooling_setpoint_c);
    if bb.autosize {
        if let Some(v) = bb.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
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

    make_spec(
        "Electric Baseboard",
        &bb.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}

// ---------------------------------------------------------------------------
// IdealHVAC
// ---------------------------------------------------------------------------

/// Ideal (load-based) HVAC with no physical equipment model. Supports autosizing when `autosize=True`.
#[pyclass(name = "IdealHVAC", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyIdealHVAC {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub heating_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub cooling_capacity_w: Option<f64>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub deadband_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyIdealHVAC {
    #[new]
    #[pyo3(signature = (name, autosize=false, zone_id=None, heating_capacity_w=None, cooling_capacity_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, deadband_c=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        zone_id: Option<u16>,
        heating_capacity_w: Option<f64>,
        cooling_capacity_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && heating_capacity_w.is_none() && cooling_capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "IdealHVAC: at least one of heating_capacity_w or cooling_capacity_w is required when autosize=False",
            ));
        }
        Ok(Self {
            name,
            autosize,
            zone_id,
            heating_capacity_w,
            cooling_capacity_w,
            heating_setpoint_c,
            cooling_setpoint_c,
            deadband_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!("IdealHVAC(name={:?})", self.name)
    }
}

pub fn ideal_hvac_spec_from_py(ih: &PyIdealHVAC) -> EquipmentSpec {
    let mut params = Map::new();

    if ih.autosize {
        params.insert("autosize_heating".into(), json!(true));
        params.insert("autosize_cooling".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", ih.heating_capacity_w);
    insert_opt(&mut params, "cooling_capacity_w", ih.cooling_capacity_w);
    insert_opt(&mut params, "zone_id", ih.zone_id);
    insert_opt(&mut params, "heating_setpoint_c", ih.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", ih.cooling_setpoint_c);
    insert_opt(&mut params, "deadband_c", ih.deadband_c);
    if ih.autosize {
        if let Some(v) = ih.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
            params.insert("autosize_cooling_factor".into(), json!(v));
        }
    }

    let typed_config = if ih.autosize {
        None
    } else {
        let cfg = IdealHvacConfig {
            heating_capacity_w: ih.heating_capacity_w,
            cooling_capacity_w: ih.cooling_capacity_w,
            zone_id: ih.zone_id,
            deadband_c: ih.deadband_c,
            setpoint: setpoint_config(ih.heating_setpoint_c, ih.cooling_setpoint_c),
            ..IdealHvacConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            ih.name.clone(),
            "Ideal HVAC".to_string(),
            cfg,
        ))
    };

    make_spec(
        "Ideal HVAC",
        &ih.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}

// ---------------------------------------------------------------------------
// GasBoiler
// ---------------------------------------------------------------------------

/// Gas-fired boiler heating equipment. Supports autosizing when `autosize=True`.
#[pyclass(name = "GasBoiler", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyGasBoiler {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub afue: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyGasBoiler {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, afue=None, zone_id=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        afue: Option<f64>,
        zone_id: Option<u16>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "GasBoiler: capacity_w is required when autosize=False",
            ));
        }
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "GasBoiler: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = afue {
            if v <= 0.0 || v > 1.0 {
                return Err(PyValueError::new_err(
                    "GasBoiler: afue must be between 0 and 1",
                ));
            }
        }
        Ok(Self {
            name,
            autosize,
            capacity_w,
            afue,
            zone_id,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "GasBoiler(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

pub fn gas_boiler_spec_from_py(gb: &PyGasBoiler) -> EquipmentSpec {
    let mut params = Map::new();

    if gb.autosize {
        params.insert("autosize_heating".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", gb.capacity_w);
    insert_opt(&mut params, "efficiency_afue", gb.afue);
    insert_opt(&mut params, "zone_id", gb.zone_id);
    insert_opt(&mut params, "heating_setpoint_c", gb.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", gb.cooling_setpoint_c);
    if gb.autosize {
        if let Some(v) = gb.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
        }
    }

    let typed_config = if gb.autosize {
        None
    } else {
        let cfg = GasBoilerConfig {
            capacity_w: gb.capacity_w.unwrap(),
            afue: gb.afue.unwrap_or(0.80),
            zone_id: gb.zone_id,
            setpoint: setpoint_config(gb.heating_setpoint_c, gb.cooling_setpoint_c),
            ..GasBoilerConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            gb.name.clone(),
            "Gas Boiler".to_string(),
            cfg,
        ))
    };

    make_spec("Gas Boiler", &gb.name, FuelType::Gas, params, typed_config)
}

// ---------------------------------------------------------------------------
// ElectricBoiler
// ---------------------------------------------------------------------------

/// Electric boiler heating equipment. Supports autosizing when `autosize=True`.
#[pyclass(name = "ElectricBoiler", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricBoiler {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub eir: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricBoiler {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, eir=None, zone_id=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    // PyO3 #[new] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
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
                "ElectricBoiler: capacity_w is required when autosize=False",
            ));
        }
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "ElectricBoiler: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = eir {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "ElectricBoiler: eir must be positive",
                ));
            }
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
            "ElectricBoiler(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

pub fn electric_boiler_spec_from_py(eb: &PyElectricBoiler) -> EquipmentSpec {
    let mut params = Map::new();
    let eir = eb.eir.unwrap_or(1.0);

    if eb.autosize {
        params.insert("autosize_heating".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", eb.capacity_w);
    params.insert("eir".into(), json!(eir));
    params.insert("heating_efficiency".into(), json!(eir));
    insert_opt(&mut params, "zone_id", eb.zone_id);
    insert_opt(&mut params, "heating_setpoint_c", eb.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", eb.cooling_setpoint_c);
    if eb.autosize {
        if let Some(v) = eb.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
        }
    }

    let typed_config = if eb.autosize {
        None
    } else {
        let cfg = ElectricBoilerConfig {
            capacity_w: eb.capacity_w.unwrap(),
            eir,
            zone_id: eb.zone_id,
            setpoint: setpoint_config(eb.heating_setpoint_c, eb.cooling_setpoint_c),
            ..ElectricBoilerConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            eb.name.clone(),
            "Electric Boiler".to_string(),
            cfg,
        ))
    };

    make_spec(
        "Electric Boiler",
        &eb.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}

// ---------------------------------------------------------------------------
// ElectricFurnace
// ---------------------------------------------------------------------------

/// Electric furnace heating equipment. Supports autosizing when `autosize=True`.
#[pyclass(name = "ElectricFurnace", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyElectricFurnace {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub autosize: bool,
    #[pyo3(get)]
    pub capacity_w: Option<f64>,
    #[pyo3(get)]
    pub eir: Option<f64>,
    #[pyo3(get)]
    pub zone_id: Option<u16>,
    #[pyo3(get)]
    pub fan_power_w: Option<f64>,
    #[pyo3(get)]
    pub heating_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub cooling_setpoint_c: Option<f64>,
    #[pyo3(get)]
    pub oversizing_factor: Option<f64>,
}

#[pymethods]
impl PyElectricFurnace {
    #[new]
    #[pyo3(signature = (name, autosize=true, capacity_w=None, eir=None, zone_id=None, fan_power_w=None, heating_setpoint_c=None, cooling_setpoint_c=None, oversizing_factor=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        name: String,
        autosize: bool,
        capacity_w: Option<f64>,
        eir: Option<f64>,
        zone_id: Option<u16>,
        fan_power_w: Option<f64>,
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        oversizing_factor: Option<f64>,
    ) -> PyResult<Self> {
        if !autosize && capacity_w.is_none() {
            return Err(PyValueError::new_err(
                "ElectricFurnace: capacity_w is required when autosize=False",
            ));
        }
        if let Some(v) = capacity_w {
            if v <= 0.0 || !v.is_finite() {
                return Err(PyValueError::new_err(
                    "ElectricFurnace: capacity_w must be positive and finite",
                ));
            }
        }
        if let Some(v) = eir {
            if v <= 0.0 {
                return Err(PyValueError::new_err(
                    "ElectricFurnace: eir must be positive",
                ));
            }
        }
        Ok(Self {
            name,
            autosize,
            capacity_w,
            eir,
            zone_id,
            fan_power_w,
            heating_setpoint_c,
            cooling_setpoint_c,
            oversizing_factor,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "ElectricFurnace(name={:?}, autosize={}, capacity_w={:?})",
            self.name, self.autosize, self.capacity_w
        )
    }
}

pub fn electric_furnace_spec_from_py(ef: &PyElectricFurnace) -> EquipmentSpec {
    let mut params = Map::new();
    let eir = ef.eir.unwrap_or(1.0);

    if ef.autosize {
        params.insert("autosize_heating".into(), json!(true));
    }
    insert_opt(&mut params, "heating_capacity_w", ef.capacity_w);
    params.insert("eir".into(), json!(eir));
    params.insert("heating_efficiency".into(), json!(eir));
    insert_opt(&mut params, "zone_id", ef.zone_id);
    insert_opt(&mut params, "fan_power_w", ef.fan_power_w);
    insert_opt(&mut params, "heating_setpoint_c", ef.heating_setpoint_c);
    insert_opt(&mut params, "cooling_setpoint_c", ef.cooling_setpoint_c);
    if ef.autosize {
        if let Some(v) = ef.oversizing_factor {
            params.insert("autosize_heating_factor".into(), json!(v));
        }
    }

    let typed_config = if ef.autosize {
        None
    } else {
        let cfg = ElectricFurnaceConfig {
            capacity_w: ef.capacity_w.unwrap(),
            eir,
            zone_id: ef.zone_id,
            fan_power_w: ef.fan_power_w,
            setpoint: setpoint_config(ef.heating_setpoint_c, ef.cooling_setpoint_c),
            ..ElectricFurnaceConfig::default()
        };
        Some(EquipmentConfig::from_typed(
            ef.name.clone(),
            "Electric Furnace".to_string(),
            cfg,
        ))
    };

    make_spec(
        "Electric Furnace",
        &ef.name,
        FuelType::Electric,
        params,
        typed_config,
    )
}
