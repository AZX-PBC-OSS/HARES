//! Python bindings for core enums and bitflags.
//!
//! Exposes Rust enum and bitflag types to Python with full variant coverage,
//! from_str classmethods, and equality/hash implementations.

use std::hash::{Hash, Hasher};

use hares_core::engine::SimStatus as CoreSimStatus;
use hares_fleet::fleet::SimStatus as FleetSimStatus;
use hares_io::ResStockVersion as IoResStockVersion;
use hares_types::{
    BatteryChemistry as RustBatteryChemistry, ChargingLevel as RustChargingLevel,
    ChargingStrategy as RustChargingStrategy, ControlCapabilities as RustControlCapabilities,
    DutyCycleComponent as RustDutyCycleComponent, EndUse as RustEndUse,
    EvConnectionState as RustEvConnectionState, ExecutionStage as RustExecutionStage,
    FluidType as RustFluidType, FuelType as RustFuelType,
    InverterPriority as RustInverterPriority, PlugInPolicy as RustPlugInPolicy,
    VehicleType as RustVehicleType,
};
use pyo3::prelude::*;
use pyo3::types::PyList;

#[pyclass(name = "EndUse", from_py_object)]
#[derive(Clone)]
pub struct PyEndUse {
    inner: RustEndUse,
}

impl PyEndUse {
    pub(crate) fn new(inner: RustEndUse) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyEndUse {
    #[classattr]
    const HVAC_HEATING: Self = Self {
        inner: RustEndUse::HVAC_HEATING,
    };

    #[classattr]
    const HVAC_COOLING: Self = Self {
        inner: RustEndUse::HVAC_COOLING,
    };

    #[classattr]
    const WATER_HEATING: Self = Self {
        inner: RustEndUse::WATER_HEATING,
    };

    #[classattr]
    const LIGHTING: Self = Self {
        inner: RustEndUse::LIGHTING,
    };

    #[classattr]
    const PLUG_LOADS: Self = Self {
        inner: RustEndUse::PLUG_LOADS,
    };

    #[classattr]
    const REFRIGERATION: Self = Self {
        inner: RustEndUse::REFRIGERATION,
    };

    #[classattr]
    const VENTILATION: Self = Self {
        inner: RustEndUse::VENTILATION,
    };

    #[classattr]
    const BATTERY: Self = Self {
        inner: RustEndUse::BATTERY,
    };

    #[classattr]
    const PV: Self = Self {
        inner: RustEndUse::PV,
    };

    #[classattr]
    const EV: Self = Self {
        inner: RustEndUse::EV,
    };

    #[classattr]
    const GENERATOR: Self = Self {
        inner: RustEndUse::GENERATOR,
    };

    #[classattr]
    const DEHUMIDIFIER: Self = Self {
        inner: RustEndUse::DEHUMIDIFIER,
    };

    #[classattr]
    const OTHER: Self = Self {
        inner: RustEndUse::OTHER,
    };

    #[staticmethod]
    fn custom(name: &str) -> Self {
        Self {
            inner: RustEndUse::custom(name.to_string()),
        }
    }

    fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    fn is_standard(&self) -> bool {
        self.inner.is_standard()
    }

    fn __repr__(&self) -> String {
        format!("EndUse('{}')", self.inner.as_str())
    }

    fn __str__(&self) -> String {
        self.inner.as_str().to_string()
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_enduse) = other.extract::<PyEndUse>() {
            Ok(self.inner == other_enduse.inner)
        } else {
            Ok(false)
        }
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.inner.as_str().hash(&mut hasher);
        hasher.finish()
    }
}

impl From<RustEndUse> for PyEndUse {
    fn from(inner: RustEndUse) -> Self {
        Self::new(inner)
    }
}

impl From<PyEndUse> for RustEndUse {
    fn from(py: PyEndUse) -> Self {
        py.inner
    }
}

#[pyclass(name = "FuelType", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyFuelType {
    Electric,
    Gas,
    Propane,
    Oil,
    NoFuel,
}

#[pymethods]
impl PyFuelType {
    #[staticmethod]
    fn electric() -> Self {
        PyFuelType::Electric
    }

    #[staticmethod]
    fn gas() -> Self {
        PyFuelType::Gas
    }

    #[staticmethod]
    fn propane() -> Self {
        PyFuelType::Propane
    }

    #[staticmethod]
    fn oil() -> Self {
        PyFuelType::Oil
    }

    #[staticmethod]
    fn no_fuel() -> Self {
        PyFuelType::NoFuel
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "electric" => Ok(PyFuelType::Electric),
            "gas" => Ok(PyFuelType::Gas),
            "propane" => Ok(PyFuelType::Propane),
            "oil" => Ok(PyFuelType::Oil),
            "none" | "nofuel" => Ok(PyFuelType::NoFuel),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid FuelType variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("FuelType.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyFuelType::Electric => "Electric".to_string(),
            PyFuelType::Gas => "Gas".to_string(),
            PyFuelType::Propane => "Propane".to_string(),
            PyFuelType::Oil => "Oil".to_string(),
            PyFuelType::NoFuel => "NoFuel".to_string(),
        }
    }
}

impl From<RustFuelType> for PyFuelType {
    fn from(fuel: RustFuelType) -> Self {
        match fuel {
            RustFuelType::Electric => PyFuelType::Electric,
            RustFuelType::Gas => PyFuelType::Gas,
            RustFuelType::Propane => PyFuelType::Propane,
            RustFuelType::Oil => PyFuelType::Oil,
            RustFuelType::None => PyFuelType::NoFuel,
        }
    }
}

impl From<PyFuelType> for RustFuelType {
    fn from(fuel: PyFuelType) -> Self {
        match fuel {
            PyFuelType::Electric => RustFuelType::Electric,
            PyFuelType::Gas => RustFuelType::Gas,
            PyFuelType::Propane => RustFuelType::Propane,
            PyFuelType::Oil => RustFuelType::Oil,
            PyFuelType::NoFuel => RustFuelType::None,
        }
    }
}

#[pyclass(name = "ExecutionStage", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyExecutionStage {
    Independent,
    Electrical,
    Thermal,
    EnvelopeResolution,
}

#[pymethods]
impl PyExecutionStage {
    #[staticmethod]
    fn independent() -> Self {
        PyExecutionStage::Independent
    }

    #[staticmethod]
    fn electrical() -> Self {
        PyExecutionStage::Electrical
    }

    #[staticmethod]
    fn thermal() -> Self {
        PyExecutionStage::Thermal
    }

    #[staticmethod]
    fn envelope_resolution() -> Self {
        PyExecutionStage::EnvelopeResolution
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "independent" => Ok(PyExecutionStage::Independent),
            "electrical" => Ok(PyExecutionStage::Electrical),
            "thermal" => Ok(PyExecutionStage::Thermal),
            "enveloperesolution" | "envelope_resolution" => {
                Ok(PyExecutionStage::EnvelopeResolution)
            }
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid ExecutionStage variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("ExecutionStage.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyExecutionStage::Independent => "Independent".to_string(),
            PyExecutionStage::Electrical => "Electrical".to_string(),
            PyExecutionStage::Thermal => "Thermal".to_string(),
            PyExecutionStage::EnvelopeResolution => "EnvelopeResolution".to_string(),
        }
    }
}

impl From<RustExecutionStage> for PyExecutionStage {
    fn from(stage: RustExecutionStage) -> Self {
        match stage {
            RustExecutionStage::Independent => PyExecutionStage::Independent,
            RustExecutionStage::Electrical => PyExecutionStage::Electrical,
            RustExecutionStage::Thermal => PyExecutionStage::Thermal,
            RustExecutionStage::EnvelopeResolution => PyExecutionStage::EnvelopeResolution,
        }
    }
}

impl From<PyExecutionStage> for RustExecutionStage {
    fn from(stage: PyExecutionStage) -> Self {
        match stage {
            PyExecutionStage::Independent => RustExecutionStage::Independent,
            PyExecutionStage::Electrical => RustExecutionStage::Electrical,
            PyExecutionStage::Thermal => RustExecutionStage::Thermal,
            PyExecutionStage::EnvelopeResolution => RustExecutionStage::EnvelopeResolution,
        }
    }
}

#[pyclass(name = "FluidType", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyFluidType {
    Water,
    Glycol,
    Refrigerant,
}

#[pymethods]
impl PyFluidType {
    #[staticmethod]
    fn water() -> Self {
        PyFluidType::Water
    }

    #[staticmethod]
    fn glycol() -> Self {
        PyFluidType::Glycol
    }

    #[staticmethod]
    fn refrigerant() -> Self {
        PyFluidType::Refrigerant
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "water" => Ok(PyFluidType::Water),
            "glycol" => Ok(PyFluidType::Glycol),
            "refrigerant" => Ok(PyFluidType::Refrigerant),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid FluidType variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("FluidType.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyFluidType::Water => "Water".to_string(),
            PyFluidType::Glycol => "Glycol".to_string(),
            PyFluidType::Refrigerant => "Refrigerant".to_string(),
        }
    }
}

impl From<RustFluidType> for PyFluidType {
    fn from(fluid: RustFluidType) -> Self {
        match fluid {
            RustFluidType::Water => PyFluidType::Water,
            RustFluidType::Glycol => PyFluidType::Glycol,
            RustFluidType::Refrigerant => PyFluidType::Refrigerant,
        }
    }
}

impl From<PyFluidType> for RustFluidType {
    fn from(fluid: PyFluidType) -> Self {
        match fluid {
            PyFluidType::Water => RustFluidType::Water,
            PyFluidType::Glycol => RustFluidType::Glycol,
            PyFluidType::Refrigerant => RustFluidType::Refrigerant,
        }
    }
}

#[pyclass(name = "InverterPriority", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyInverterPriority {
    Watt,
    Var,
    Cpf,
}

#[pymethods]
impl PyInverterPriority {
    #[staticmethod]
    fn watt() -> Self {
        PyInverterPriority::Watt
    }

    #[staticmethod]
    fn var() -> Self {
        PyInverterPriority::Var
    }

    #[staticmethod]
    fn cpf() -> Self {
        PyInverterPriority::Cpf
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "watt" => Ok(PyInverterPriority::Watt),
            "var" => Ok(PyInverterPriority::Var),
            "cpf" => Ok(PyInverterPriority::Cpf),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid InverterPriority variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("InverterPriority.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyInverterPriority::Watt => "Watt".to_string(),
            PyInverterPriority::Var => "Var".to_string(),
            PyInverterPriority::Cpf => "Cpf".to_string(),
        }
    }
}

impl From<RustInverterPriority> for PyInverterPriority {
    fn from(priority: RustInverterPriority) -> Self {
        match priority {
            RustInverterPriority::Watt => PyInverterPriority::Watt,
            RustInverterPriority::Var => PyInverterPriority::Var,
            RustInverterPriority::Cpf => PyInverterPriority::Cpf,
        }
    }
}

impl From<PyInverterPriority> for RustInverterPriority {
    fn from(priority: PyInverterPriority) -> Self {
        match priority {
            PyInverterPriority::Watt => RustInverterPriority::Watt,
            PyInverterPriority::Var => RustInverterPriority::Var,
            PyInverterPriority::Cpf => RustInverterPriority::Cpf,
        }
    }
}

#[pyclass(name = "DutyCycleComponent", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyDutyCycleComponent {
    Compressor,
    BackupElement,
}

#[pymethods]
impl PyDutyCycleComponent {
    #[staticmethod]
    fn compressor() -> Self {
        PyDutyCycleComponent::Compressor
    }

    #[staticmethod]
    fn backup_element() -> Self {
        PyDutyCycleComponent::BackupElement
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "compressor" => Ok(PyDutyCycleComponent::Compressor),
            "backupelement" | "backup_element" => Ok(PyDutyCycleComponent::BackupElement),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid DutyCycleComponent variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("DutyCycleComponent.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyDutyCycleComponent::Compressor => "Compressor".to_string(),
            PyDutyCycleComponent::BackupElement => "BackupElement".to_string(),
        }
    }
}

impl From<RustDutyCycleComponent> for PyDutyCycleComponent {
    fn from(component: RustDutyCycleComponent) -> Self {
        match component {
            RustDutyCycleComponent::Compressor => PyDutyCycleComponent::Compressor,
            RustDutyCycleComponent::BackupElement => PyDutyCycleComponent::BackupElement,
        }
    }
}

impl From<PyDutyCycleComponent> for RustDutyCycleComponent {
    fn from(component: PyDutyCycleComponent) -> Self {
        match component {
            PyDutyCycleComponent::Compressor => RustDutyCycleComponent::Compressor,
            PyDutyCycleComponent::BackupElement => RustDutyCycleComponent::BackupElement,
        }
    }
}

#[pyclass(name = "SimStatus", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub enum PySimStatus {
    Success(Option<String>),
    Flagged(Option<String>),
    Failed(Option<String>),
}

#[pymethods]
impl PySimStatus {
    #[staticmethod]
    fn ok() -> Self {
        PySimStatus::Success(None)
    }

    #[staticmethod]
    fn flagged(message: Option<String>) -> Self {
        PySimStatus::Flagged(message)
    }

    #[staticmethod]
    fn failed(message: Option<String>) -> Self {
        PySimStatus::Failed(message)
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "ok" => Ok(PySimStatus::Success(None)),
            "flagged" => Ok(PySimStatus::Flagged(None)),
            "failed" => Ok(PySimStatus::Failed(None)),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid SimStatus variant: {}",
                name
            ))),
        }
    }

    #[getter]
    fn message(&self) -> Option<String> {
        match self {
            PySimStatus::Success(msg) => msg.clone(),
            PySimStatus::Flagged(msg) => msg.clone(),
            PySimStatus::Failed(msg) => msg.clone(),
        }
    }

    fn __repr__(&self) -> String {
        match self {
            PySimStatus::Success(msg) => {
                if let Some(m) = msg {
                    format!("SimStatus.Success('{}')", m)
                } else {
                    "SimStatus.Ok".to_string()
                }
            }
            PySimStatus::Flagged(msg) => {
                if let Some(m) = msg {
                    format!("SimStatus.Flagged('{}')", m)
                } else {
                    "SimStatus.Flagged".to_string()
                }
            }
            PySimStatus::Failed(msg) => {
                if let Some(m) = msg {
                    format!("SimStatus.Failed('{}')", m)
                } else {
                    "SimStatus.Failed".to_string()
                }
            }
        }
    }

    fn __str__(&self) -> String {
        match self {
            PySimStatus::Success(msg) => {
                if let Some(m) = msg {
                    format!("Success('{}')", m)
                } else {
                    "Ok".to_string()
                }
            }
            PySimStatus::Flagged(msg) => {
                if let Some(m) = msg {
                    format!("Flagged('{}')", m)
                } else {
                    "Flagged".to_string()
                }
            }
            PySimStatus::Failed(msg) => {
                if let Some(m) = msg {
                    format!("Failed('{}')", m)
                } else {
                    "Failed".to_string()
                }
            }
        }
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_status) = other.extract::<PySimStatus>() {
            Ok(self == &other_status)
        } else {
            Ok(false)
        }
    }

    fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        match self {
            PySimStatus::Success(msg) => {
                0u8.hash(&mut hasher);
                if let Some(m) = msg {
                    m.hash(&mut hasher);
                }
            }
            PySimStatus::Flagged(msg) => {
                1u8.hash(&mut hasher);
                if let Some(m) = msg {
                    m.hash(&mut hasher);
                }
            }
            PySimStatus::Failed(msg) => {
                2u8.hash(&mut hasher);
                if let Some(m) = msg {
                    m.hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }
}

impl From<CoreSimStatus> for PySimStatus {
    fn from(status: CoreSimStatus) -> Self {
        match status {
            CoreSimStatus::Ok => PySimStatus::Success(None),
            CoreSimStatus::Flagged(msg) => PySimStatus::Flagged(Some(msg)),
            CoreSimStatus::Failed(msg) => PySimStatus::Failed(Some(msg)),
        }
    }
}

impl From<FleetSimStatus> for PySimStatus {
    fn from(status: FleetSimStatus) -> Self {
        match status {
            FleetSimStatus::Ok => PySimStatus::Success(None),
            FleetSimStatus::Flagged(msg) => PySimStatus::Flagged(Some(msg)),
            FleetSimStatus::Failed(msg) => PySimStatus::Failed(Some(msg)),
        }
    }
}

#[pyclass(name = "AggregationResolution", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyAggregationResolution {
    FifteenMin,
    Hourly,
}

#[pymethods]
impl PyAggregationResolution {
    #[staticmethod]
    fn fifteen_min() -> Self {
        PyAggregationResolution::FifteenMin
    }

    #[staticmethod]
    fn hourly() -> Self {
        PyAggregationResolution::Hourly
    }

    #[staticmethod]
    pub fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "15min" | "fifteenmin" | "fifteen_min" => Ok(PyAggregationResolution::FifteenMin),
            "hourly" => Ok(PyAggregationResolution::Hourly),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid AggregationResolution variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("AggregationResolution.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyAggregationResolution::FifteenMin => "FifteenMin".to_string(),
            PyAggregationResolution::Hourly => "Hourly".to_string(),
        }
    }
}

#[pyclass(name = "ResStockVersion", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyResStockVersion {
    V2024_1,
    V2024_2,
    V2025_1,
}

#[pymethods]
impl PyResStockVersion {
    #[staticmethod]
    fn v2024_1() -> Self {
        PyResStockVersion::V2024_1
    }

    #[staticmethod]
    fn v2024_2() -> Self {
        PyResStockVersion::V2024_2
    }

    #[staticmethod]
    fn v2025_1() -> Self {
        PyResStockVersion::V2025_1
    }

    #[staticmethod]
    pub fn from_str(name: &str) -> PyResult<Self> {
        let normalized: String = name
            .to_lowercase()
            .chars()
            .filter(|c| *c != '.' && *c != '_')
            .collect();
        match normalized.as_str() {
            "20241" | "v20241" => Ok(PyResStockVersion::V2024_1),
            "20242" | "v20242" => Ok(PyResStockVersion::V2024_2),
            "20251" | "v20251" => Ok(PyResStockVersion::V2025_1),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid ResStockVersion variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("ResStockVersion.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyResStockVersion::V2024_1 => "V2024_1".to_string(),
            PyResStockVersion::V2024_2 => "V2024_2".to_string(),
            PyResStockVersion::V2025_1 => "V2025_1".to_string(),
        }
    }
}

impl From<IoResStockVersion> for PyResStockVersion {
    fn from(version: IoResStockVersion) -> Self {
        match version {
            IoResStockVersion::V2024_1 => PyResStockVersion::V2024_1,
            IoResStockVersion::V2024_2 => PyResStockVersion::V2024_2,
            IoResStockVersion::V2025_1 => PyResStockVersion::V2025_1,
        }
    }
}

impl From<PyResStockVersion> for IoResStockVersion {
    fn from(version: PyResStockVersion) -> Self {
        match version {
            PyResStockVersion::V2024_1 => IoResStockVersion::V2024_1,
            PyResStockVersion::V2024_2 => IoResStockVersion::V2024_2,
            PyResStockVersion::V2025_1 => IoResStockVersion::V2025_1,
        }
    }
}

#[pyclass(name = "ControlCapabilities", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PyControlCapabilities {
    inner: RustControlCapabilities,
}

impl PyControlCapabilities {
    pub(crate) fn new(inner: RustControlCapabilities) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyControlCapabilities {
    #[classattr]
    const POWER_SETPOINT: Self = Self {
        inner: RustControlCapabilities::POWER_SETPOINT,
    };

    #[classattr]
    const SOC_TARGET: Self = Self {
        inner: RustControlCapabilities::SOC_TARGET,
    };

    #[classattr]
    const THERMAL_SETPOINT: Self = Self {
        inner: RustControlCapabilities::THERMAL_SETPOINT,
    };

    #[classattr]
    const POWER_LIMIT: Self = Self {
        inner: RustControlCapabilities::POWER_LIMIT,
    };

    #[classattr]
    const MODE_OVERRIDE: Self = Self {
        inner: RustControlCapabilities::MODE_OVERRIDE,
    };

    #[classattr]
    const DUTY_CYCLE: Self = Self {
        inner: RustControlCapabilities::DUTY_CYCLE,
    };

    #[classattr]
    const LOAD_FRACTION: Self = Self {
        inner: RustControlCapabilities::LOAD_FRACTION,
    };

    #[classattr]
    const GRID_CONNECT: Self = Self {
        inner: RustControlCapabilities::GRID_CONNECT,
    };

    #[classattr]
    const SELF_CONSUMPTION: Self = Self {
        inner: RustControlCapabilities::SELF_CONSUMPTION,
    };

    #[classattr]
    const DEMAND_RESPONSE: Self = Self {
        inner: RustControlCapabilities::DEMAND_RESPONSE,
    };

    #[classattr]
    const PROTOCOL_NATIVE: Self = Self {
        inner: RustControlCapabilities::PROTOCOL_NATIVE,
    };

    #[classattr]
    const HUMIDITY_SETPOINT: Self = Self {
        inner: RustControlCapabilities::HUMIDITY_SETPOINT,
    };

    #[classattr]
    const CURTAILMENT_PERCENT: Self = Self {
        inner: RustControlCapabilities::CURTAILMENT_PERCENT,
    };

    #[classattr]
    const REACTIVE_SETPOINT: Self = Self {
        inner: RustControlCapabilities::REACTIVE_SETPOINT,
    };

    #[classattr]
    const POWER_FACTOR_SETPOINT: Self = Self {
        inner: RustControlCapabilities::POWER_FACTOR_SETPOINT,
    };

    #[classattr]
    const INVERTER_PRIORITY_MODE: Self = Self {
        inner: RustControlCapabilities::INVERTER_PRIORITY_MODE,
    };

    #[staticmethod]
    fn from_list(_py: Python<'_>, names: &Bound<'_, PyList>) -> PyResult<Self> {
        let mut caps = RustControlCapabilities::empty();
        for item in names.iter() {
            let name: String = item.extract()?;
            match name.to_uppercase().as_str() {
                "POWER_SETPOINT" => caps |= RustControlCapabilities::POWER_SETPOINT,
                "SOC_TARGET" => caps |= RustControlCapabilities::SOC_TARGET,
                "THERMAL_SETPOINT" => caps |= RustControlCapabilities::THERMAL_SETPOINT,
                "POWER_LIMIT" => caps |= RustControlCapabilities::POWER_LIMIT,
                "MODE_OVERRIDE" => caps |= RustControlCapabilities::MODE_OVERRIDE,
                "DUTY_CYCLE" => caps |= RustControlCapabilities::DUTY_CYCLE,
                "LOAD_FRACTION" => caps |= RustControlCapabilities::LOAD_FRACTION,
                "GRID_CONNECT" => caps |= RustControlCapabilities::GRID_CONNECT,
                "SELF_CONSUMPTION" => caps |= RustControlCapabilities::SELF_CONSUMPTION,
                "DEMAND_RESPONSE" => caps |= RustControlCapabilities::DEMAND_RESPONSE,
                "PROTOCOL_NATIVE" => caps |= RustControlCapabilities::PROTOCOL_NATIVE,
                "HUMIDITY_SETPOINT" => caps |= RustControlCapabilities::HUMIDITY_SETPOINT,
                "CURTAILMENT_PERCENT" => caps |= RustControlCapabilities::CURTAILMENT_PERCENT,
                "REACTIVE_SETPOINT" => caps |= RustControlCapabilities::REACTIVE_SETPOINT,
                "POWER_FACTOR_SETPOINT" => caps |= RustControlCapabilities::POWER_FACTOR_SETPOINT,
                "INVERTER_PRIORITY_MODE" => caps |= RustControlCapabilities::INVERTER_PRIORITY_MODE,
                _ => {
                    return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "invalid ControlCapabilities flag: {}",
                        name
                    )));
                }
            }
        }
        Ok(Self { inner: caps })
    }

    fn __or__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        if let Ok(other_caps) = other.extract::<PyControlCapabilities>() {
            Ok(Self {
                inner: self.inner | other_caps.inner,
            })
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "expected ControlCapabilities",
            ))
        }
    }

    fn __contains__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_caps) = other.extract::<PyControlCapabilities>() {
            Ok(self.inner.contains(other_caps.inner))
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "expected ControlCapabilities",
            ))
        }
    }

    fn __repr__(&self) -> String {
        let mut flags = Vec::new();
        if self.inner.contains(RustControlCapabilities::POWER_SETPOINT) {
            flags.push("POWER_SETPOINT");
        }
        if self.inner.contains(RustControlCapabilities::SOC_TARGET) {
            flags.push("SOC_TARGET");
        }
        if self
            .inner
            .contains(RustControlCapabilities::THERMAL_SETPOINT)
        {
            flags.push("THERMAL_SETPOINT");
        }
        if self.inner.contains(RustControlCapabilities::POWER_LIMIT) {
            flags.push("POWER_LIMIT");
        }
        if self.inner.contains(RustControlCapabilities::MODE_OVERRIDE) {
            flags.push("MODE_OVERRIDE");
        }
        if self.inner.contains(RustControlCapabilities::DUTY_CYCLE) {
            flags.push("DUTY_CYCLE");
        }
        if self.inner.contains(RustControlCapabilities::LOAD_FRACTION) {
            flags.push("LOAD_FRACTION");
        }
        if self.inner.contains(RustControlCapabilities::GRID_CONNECT) {
            flags.push("GRID_CONNECT");
        }
        if self
            .inner
            .contains(RustControlCapabilities::SELF_CONSUMPTION)
        {
            flags.push("SELF_CONSUMPTION");
        }
        if self
            .inner
            .contains(RustControlCapabilities::DEMAND_RESPONSE)
        {
            flags.push("DEMAND_RESPONSE");
        }
        if self
            .inner
            .contains(RustControlCapabilities::PROTOCOL_NATIVE)
        {
            flags.push("PROTOCOL_NATIVE");
        }
        if self
            .inner
            .contains(RustControlCapabilities::HUMIDITY_SETPOINT)
        {
            flags.push("HUMIDITY_SETPOINT");
        }
        if self
            .inner
            .contains(RustControlCapabilities::CURTAILMENT_PERCENT)
        {
            flags.push("CURTAILMENT_PERCENT");
        }
        if self
            .inner
            .contains(RustControlCapabilities::REACTIVE_SETPOINT)
        {
            flags.push("REACTIVE_SETPOINT");
        }
        if self
            .inner
            .contains(RustControlCapabilities::POWER_FACTOR_SETPOINT)
        {
            flags.push("POWER_FACTOR_SETPOINT");
        }
        if self
            .inner
            .contains(RustControlCapabilities::INVERTER_PRIORITY_MODE)
        {
            flags.push("INVERTER_PRIORITY_MODE");
        }
        if flags.is_empty() {
            "ControlCapabilities(0)".to_string()
        } else {
            format!("ControlCapabilities({})", flags.join(" | "))
        }
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }
}

impl From<RustControlCapabilities> for PyControlCapabilities {
    fn from(caps: RustControlCapabilities) -> Self {
        Self::new(caps)
    }
}

impl From<PyControlCapabilities> for RustControlCapabilities {
    fn from(caps: PyControlCapabilities) -> Self {
        caps.inner
    }
}

#[pyclass(name = "LutType", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyLutType {
    ChargingCurve,
    Ocv,
    UNeg,
}

#[pymethods]
impl PyLutType {
    #[staticmethod]
    fn charging_curve() -> Self {
        PyLutType::ChargingCurve
    }

    #[staticmethod]
    fn ocv() -> Self {
        PyLutType::Ocv
    }

    #[staticmethod]
    fn uneg() -> Self {
        PyLutType::UNeg
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "chargingcurve" | "charging_curve" => Ok(PyLutType::ChargingCurve),
            "ocv" => Ok(PyLutType::Ocv),
            "uneg" => Ok(PyLutType::UNeg),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid LutType variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("LutType.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyLutType::ChargingCurve => "ChargingCurve".to_string(),
            PyLutType::Ocv => "Ocv".to_string(),
            PyLutType::UNeg => "UNeg".to_string(),
        }
    }
}

#[pyclass(name = "BatteryChemistry", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyBatteryChemistry {
    Nmc,
    Lfp,
    Nca,
    Lto,
}

#[pymethods]
impl PyBatteryChemistry {
    #[staticmethod]
    fn nmc() -> Self {
        PyBatteryChemistry::Nmc
    }

    #[staticmethod]
    fn lfp() -> Self {
        PyBatteryChemistry::Lfp
    }

    #[staticmethod]
    fn nca() -> Self {
        PyBatteryChemistry::Nca
    }

    #[staticmethod]
    fn lto() -> Self {
        PyBatteryChemistry::Lto
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustBatteryChemistry>()
            .map(PyBatteryChemistry::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    fn __repr__(&self) -> String {
        format!("BatteryChemistry.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustBatteryChemistry::from(*self).to_string()
    }
}

impl From<RustBatteryChemistry> for PyBatteryChemistry {
    fn from(c: RustBatteryChemistry) -> Self {
        match c {
            RustBatteryChemistry::Nmc => Self::Nmc,
            RustBatteryChemistry::Lfp => Self::Lfp,
            RustBatteryChemistry::Nca => Self::Nca,
            RustBatteryChemistry::Lto => Self::Lto,
        }
    }
}

impl From<PyBatteryChemistry> for RustBatteryChemistry {
    fn from(c: PyBatteryChemistry) -> Self {
        match c {
            PyBatteryChemistry::Nmc => Self::Nmc,
            PyBatteryChemistry::Lfp => Self::Lfp,
            PyBatteryChemistry::Nca => Self::Nca,
            PyBatteryChemistry::Lto => Self::Lto,
        }
    }
}

#[pyclass(name = "ChargingLevel", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyChargingLevel {
    L1,
    L2,
}

#[pymethods]
impl PyChargingLevel {
    #[staticmethod]
    fn l1() -> Self {
        PyChargingLevel::L1
    }

    #[staticmethod]
    fn l2() -> Self {
        PyChargingLevel::L2
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustChargingLevel>()
            .map(PyChargingLevel::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    fn __repr__(&self) -> String {
        format!("ChargingLevel.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustChargingLevel::from(*self).to_string()
    }
}

impl From<RustChargingLevel> for PyChargingLevel {
    fn from(c: RustChargingLevel) -> Self {
        match c {
            RustChargingLevel::L1 => Self::L1,
            RustChargingLevel::L2 => Self::L2,
        }
    }
}

impl From<PyChargingLevel> for RustChargingLevel {
    fn from(c: PyChargingLevel) -> Self {
        match c {
            PyChargingLevel::L1 => Self::L1,
            PyChargingLevel::L2 => Self::L2,
        }
    }
}

#[pyclass(name = "VehicleType", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyVehicleType {
    Bev,
    Phev,
}

#[pymethods]
impl PyVehicleType {
    #[staticmethod]
    fn bev() -> Self {
        PyVehicleType::Bev
    }

    #[staticmethod]
    fn phev() -> Self {
        PyVehicleType::Phev
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustVehicleType>()
            .map(PyVehicleType::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    fn __repr__(&self) -> String {
        format!("VehicleType.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustVehicleType::from(*self).to_string()
    }
}

impl From<RustVehicleType> for PyVehicleType {
    fn from(v: RustVehicleType) -> Self {
        match v {
            RustVehicleType::Bev => Self::Bev,
            RustVehicleType::Phev => Self::Phev,
        }
    }
}

impl From<PyVehicleType> for RustVehicleType {
    fn from(v: PyVehicleType) -> Self {
        match v {
            PyVehicleType::Bev => Self::Bev,
            PyVehicleType::Phev => Self::Phev,
        }
    }
}

#[pyclass(name = "EvConnectionState", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyEvConnectionState {
    HomePluggedIn,
    AwayPluggedIn,
    Disconnected,
}

#[pymethods]
impl PyEvConnectionState {
    #[staticmethod]
    fn home_plugged_in() -> Self {
        PyEvConnectionState::HomePluggedIn
    }

    #[staticmethod]
    fn away_plugged_in() -> Self {
        PyEvConnectionState::AwayPluggedIn
    }

    #[staticmethod]
    fn disconnected() -> Self {
        PyEvConnectionState::Disconnected
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustEvConnectionState>()
            .map(PyEvConnectionState::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    fn __repr__(&self) -> String {
        format!("EvConnectionState.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustEvConnectionState::from(*self).to_string()
    }

    fn __hash__(&self) -> u64 {
        *self as u64
    }
}

impl From<RustEvConnectionState> for PyEvConnectionState {
    fn from(s: RustEvConnectionState) -> Self {
        match s {
            RustEvConnectionState::HomePluggedIn => Self::HomePluggedIn,
            RustEvConnectionState::AwayPluggedIn => Self::AwayPluggedIn,
            RustEvConnectionState::Disconnected => Self::Disconnected,
        }
    }
}

impl From<PyEvConnectionState> for RustEvConnectionState {
    fn from(s: PyEvConnectionState) -> Self {
        match s {
            PyEvConnectionState::HomePluggedIn => Self::HomePluggedIn,
            PyEvConnectionState::AwayPluggedIn => Self::AwayPluggedIn,
            PyEvConnectionState::Disconnected => Self::Disconnected,
        }
    }
}

fn validate_soc(name: &str, value: f64) -> PyResult<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "{name} must be in [0.0, 1.0], got {value}"
        )));
    }
    Ok(())
}

fn validate_hour(name: &str, value: f64) -> PyResult<()> {
    if !value.is_finite() || !(0.0..24.0).contains(&value) {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
            "{name} must be in [0.0, 24.0), got {value}"
        )));
    }
    Ok(())
}

#[pyclass(name = "PlugInPolicy", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub struct PyPlugInPolicy {
    pub inner: RustPlugInPolicy,
}

#[pymethods]
impl PyPlugInPolicy {
    #[staticmethod]
    fn always() -> Self {
        Self { inner: RustPlugInPolicy::Always }
    }

    #[staticmethod]
    fn low_soc(threshold: f64) -> PyResult<Self> {
        validate_soc("threshold", threshold)?;
        Ok(Self { inner: RustPlugInPolicy::LowSoc { threshold } })
    }

    fn __repr__(&self) -> String {
        format!("{:?}", self.inner)
    }

    fn __str__(&self) -> String {
        match &self.inner {
            RustPlugInPolicy::Always => "Always".to_string(),
            RustPlugInPolicy::LowSoc { threshold } => format!("LowSoc(threshold={threshold})"),
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

#[pyclass(name = "ChargingStrategy", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub struct PyChargingStrategy {
    pub inner: RustChargingStrategy,
}

#[pymethods]
impl PyChargingStrategy {
    #[staticmethod]
    fn immediate(target_soc: f64) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        Ok(Self { inner: RustChargingStrategy::Immediate { target_soc } })
    }

    #[staticmethod]
    fn nightly(off_peak_start_hour: f64, off_peak_end_hour: f64, target_soc: f64) -> PyResult<Self> {
        validate_hour("off_peak_start_hour", off_peak_start_hour)?;
        validate_hour("off_peak_end_hour", off_peak_end_hour)?;
        validate_soc("target_soc", target_soc)?;
        Ok(Self {
            inner: RustChargingStrategy::Nightly {
                off_peak_start_hour,
                off_peak_end_hour,
                target_soc,
            },
        })
    }

    #[staticmethod]
    fn low_soc(threshold: f64, target_soc: f64) -> PyResult<Self> {
        validate_soc("threshold", threshold)?;
        validate_soc("target_soc", target_soc)?;
        Ok(Self { inner: RustChargingStrategy::LowSoc { threshold, target_soc } })
    }

    #[staticmethod]
    fn quick_then_wait(partial_soc: f64) -> PyResult<Self> {
        validate_soc("partial_soc", partial_soc)?;
        Ok(Self { inner: RustChargingStrategy::QuickThenWait { partial_soc } })
    }

    #[staticmethod]
    fn pre_departure(target_soc: f64) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        Ok(Self {
            inner: RustChargingStrategy::PreDeparture {
                target_soc,
                departure_schedule: Vec::new(),
            },
        })
    }

    #[staticmethod]
    fn tou_aware(target_soc: f64) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        Ok(Self {
            inner: RustChargingStrategy::TouAware {
                target_soc,
                departure_schedule: Vec::new(),
                charge_buffer_hours: 2.0,
            },
        })
    }

    #[staticmethod]
    fn v2h(discharge_threshold_soc: f64, min_soc: f64) -> PyResult<Self> {
        validate_soc("discharge_threshold_soc", discharge_threshold_soc)?;
        validate_soc("min_soc", min_soc)?;
        if min_soc > discharge_threshold_soc {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "min_soc must be <= discharge_threshold_soc",
            ));
        }
        Ok(Self {
            inner: RustChargingStrategy::V2H {
                discharge_threshold_soc,
                min_soc,
            },
        })
    }

    #[staticmethod]
    fn v2g(min_soc: f64, max_export_kw: f64, price_threshold: f64) -> PyResult<Self> {
        validate_soc("min_soc", min_soc)?;
        if !max_export_kw.is_finite() || max_export_kw < 0.0 {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "max_export_kw must be finite and >= 0, got {max_export_kw}"
            )));
        }
        if !price_threshold.is_finite() {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "price_threshold must be finite".to_string(),
            ));
        }
        Ok(Self {
            inner: RustChargingStrategy::V2G {
                min_soc,
                max_export_kw,
                price_threshold,
            },
        })
    }

    #[staticmethod]
    fn solar_surplus(min_charge_rate_kw: f64) -> PyResult<Self> {
        if !min_charge_rate_kw.is_finite() || min_charge_rate_kw < 0.0 {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "min_charge_rate_kw must be finite and >= 0, got {min_charge_rate_kw}"
            )));
        }
        Ok(Self {
            inner: RustChargingStrategy::SolarSurplus {
                min_charge_rate_kw,
                departure_schedule: Vec::new(),
            },
        })
    }

    fn __repr__(&self) -> String {
        format!("{:?}", self.inner)
    }

    fn __str__(&self) -> String {
        match &self.inner {
            RustChargingStrategy::Immediate { target_soc } => {
                format!("Immediate(target_soc={target_soc})")
            }
            RustChargingStrategy::Nightly { off_peak_start_hour, off_peak_end_hour, target_soc } => {
                format!("Nightly(off_peak_start={off_peak_start_hour}, off_peak_end={off_peak_end_hour}, target_soc={target_soc})")
            }
            RustChargingStrategy::LowSoc { threshold, target_soc } => {
                format!("LowSoc(threshold={threshold}, target_soc={target_soc})")
            }
            RustChargingStrategy::QuickThenWait { partial_soc } => {
                format!("QuickThenWait(partial_soc={partial_soc})")
            }
            RustChargingStrategy::PreDeparture { target_soc, .. } => {
                format!("PreDeparture(target_soc={target_soc})")
            }
            RustChargingStrategy::TouAware { target_soc, charge_buffer_hours, .. } => {
                format!("TouAware(target_soc={target_soc}, buffer_hours={charge_buffer_hours})")
            }
            RustChargingStrategy::SolarSurplus { min_charge_rate_kw, .. } => {
                format!("SolarSurplus(min_charge_rate_kw={min_charge_rate_kw})")
            }
            RustChargingStrategy::V2H { discharge_threshold_soc, min_soc } => {
                format!("V2H(discharge_threshold_soc={discharge_threshold_soc}, min_soc={min_soc})")
            }
            RustChargingStrategy::V2G { min_soc, max_export_kw, price_threshold } => {
                format!("V2G(min_soc={min_soc}, max_export_kw={max_export_kw}, price_threshold={price_threshold})")
            }
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
