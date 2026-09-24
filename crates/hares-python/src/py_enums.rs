//! Python bindings for core enums and bitflags.
//!
//! Exposes Rust enum and bitflag types to Python with full variant coverage,
//! from_str classmethods, and equality/hash implementations.

use std::hash::{Hash, Hasher};

use hares_core::engine::SimStatus as CoreSimStatus;
use hares_equipment::battery::catalog::BatteryProductId as RustBatteryProductId;
use hares_equipment::ev::catalog::{
    EvArchetypeId as RustEvArchetypeId, VehicleId as RustVehicleId,
};
use hares_equipment::ndinterp::ExtrapolationStrategy as RustExtrapolationStrategy;
use hares_fleet::fleet::SimStatus as FleetSimStatus;
use hares_io::ResStockVersion as IoResStockVersion;
use hares_types::{
    BatteryChemistry as RustBatteryChemistry, BmsAction as RustBmsAction, BmsMode as RustBmsMode,
    BmsScheduleWindow as RustBmsScheduleWindow, BmsTimeWindow as RustBmsTimeWindow,
    ChargingLevel as RustChargingLevel, ChargingStrategy as RustChargingStrategy,
    ControlCapabilities as RustControlCapabilities, DayFilter,
    DepartureConstraint as RustDepartureConstraint, DutyCycleComponent as RustDutyCycleComponent,
    EndUse as RustEndUse, EvConnectionState as RustEvConnectionState,
    ExecutionStage as RustExecutionStage, FluidType as RustFluidType, FuelType as RustFuelType,
    GridExportRule as RustGridExportRule, IdealCapacityMode as RustIdealCapacityMode,
    InverterPriority as RustInverterPriority, PlugInPolicy as RustPlugInPolicy,
    StormWatchTrigger as RustStormWatchTrigger, VehicleType as RustVehicleType,
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

    #[classattr]
    const COOKING: Self = Self {
        inner: RustEndUse::COOKING,
    };

    #[classattr]
    const LAUNDRY: Self = Self {
        inner: RustEndUse::LAUNDRY,
    };

    #[classattr]
    const DISHWASHER: Self = Self {
        inner: RustEndUse::DISHWASHER,
    };

    #[classattr]
    const POOL_PUMP: Self = Self {
        inner: RustEndUse::POOL_PUMP,
    };

    #[classattr]
    const POOL_HEATER: Self = Self {
        inner: RustEndUse::POOL_HEATER,
    };

    #[classattr]
    const SPA_PUMP: Self = Self {
        inner: RustEndUse::SPA_PUMP,
    };

    #[classattr]
    const SPA_HEATER: Self = Self {
        inner: RustEndUse::SPA_HEATER,
    };

    #[classattr]
    const CEILING_FAN: Self = Self {
        inner: RustEndUse::CEILING_FAN,
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
    Wood,
    Coal,
    WoodPellet,
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
    fn wood() -> Self {
        PyFuelType::Wood
    }

    #[staticmethod]
    fn coal() -> Self {
        PyFuelType::Coal
    }

    #[staticmethod]
    fn wood_pellet() -> Self {
        PyFuelType::WoodPellet
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
            "wood" => Ok(PyFuelType::Wood),
            "coal" => Ok(PyFuelType::Coal),
            "woodpellet" | "wood_pellet" | "wood pellets" => Ok(PyFuelType::WoodPellet),
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
            PyFuelType::Wood => "Wood".to_string(),
            PyFuelType::Coal => "Coal".to_string(),
            PyFuelType::WoodPellet => "WoodPellet".to_string(),
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
            RustFuelType::Wood => PyFuelType::Wood,
            RustFuelType::Coal => PyFuelType::Coal,
            RustFuelType::WoodPellet => PyFuelType::WoodPellet,
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
            PyFuelType::Wood => RustFuelType::Wood,
            PyFuelType::Coal => RustFuelType::Coal,
            PyFuelType::WoodPellet => RustFuelType::WoodPellet,
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

#[pyclass(name = "IdealCapacityMode", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyIdealCapacityMode {
    Auto,
    On,
    Off,
}

#[pymethods]
impl PyIdealCapacityMode {
    #[staticmethod]
    fn auto() -> Self {
        PyIdealCapacityMode::Auto
    }

    #[staticmethod]
    fn on() -> Self {
        PyIdealCapacityMode::On
    }

    #[staticmethod]
    fn off() -> Self {
        PyIdealCapacityMode::Off
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "auto" => Ok(PyIdealCapacityMode::Auto),
            "on" => Ok(PyIdealCapacityMode::On),
            "off" => Ok(PyIdealCapacityMode::Off),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid IdealCapacityMode variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("IdealCapacityMode.{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyIdealCapacityMode::Auto => "Auto".to_string(),
            PyIdealCapacityMode::On => "On".to_string(),
            PyIdealCapacityMode::Off => "Off".to_string(),
        }
    }
}

impl From<RustIdealCapacityMode> for PyIdealCapacityMode {
    fn from(mode: RustIdealCapacityMode) -> Self {
        match mode {
            RustIdealCapacityMode::Auto => PyIdealCapacityMode::Auto,
            RustIdealCapacityMode::On => PyIdealCapacityMode::On,
            RustIdealCapacityMode::Off => PyIdealCapacityMode::Off,
        }
    }
}

impl From<PyIdealCapacityMode> for RustIdealCapacityMode {
    fn from(mode: PyIdealCapacityMode) -> Self {
        match mode {
            PyIdealCapacityMode::Auto => RustIdealCapacityMode::Auto,
            PyIdealCapacityMode::On => RustIdealCapacityMode::On,
            PyIdealCapacityMode::Off => RustIdealCapacityMode::Off,
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

    #[classattr]
    const IDEAL_CAPACITY: Self = Self {
        inner: RustControlCapabilities::IDEAL_CAPACITY,
    };

    #[classattr]
    const IDEAL_CAPACITY_MODE_OVERRIDE: Self = Self {
        inner: RustControlCapabilities::IDEAL_CAPACITY_MODE_OVERRIDE,
    };

    #[classattr]
    const EV_PLUG_IN: Self = Self {
        inner: RustControlCapabilities::EV_PLUG_IN,
    };

    #[classattr]
    const EV_DRIVE: Self = Self {
        inner: RustControlCapabilities::EV_DRIVE,
    };

    #[classattr]
    const EV_AWAY_CHARGE: Self = Self {
        inner: RustControlCapabilities::EV_AWAY_CHARGE,
    };

    #[classattr]
    const EV_SET_READY_BY: Self = Self {
        inner: RustControlCapabilities::EV_SET_READY_BY,
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
                "IDEAL_CAPACITY" => caps |= RustControlCapabilities::IDEAL_CAPACITY,
                "IDEAL_CAPACITY_MODE_OVERRIDE" => {
                    caps |= RustControlCapabilities::IDEAL_CAPACITY_MODE_OVERRIDE
                }
                "EV_PLUG_IN" => caps |= RustControlCapabilities::EV_PLUG_IN,
                "EV_DRIVE" => caps |= RustControlCapabilities::EV_DRIVE,
                "EV_AWAY_CHARGE" => caps |= RustControlCapabilities::EV_AWAY_CHARGE,
                "EV_SET_READY_BY" => caps |= RustControlCapabilities::EV_SET_READY_BY,
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
        Self {
            inner: RustPlugInPolicy::Always,
        }
    }

    #[staticmethod]
    fn low_soc(threshold: f64) -> PyResult<Self> {
        validate_soc("threshold", threshold)?;
        Ok(Self {
            inner: RustPlugInPolicy::LowSoc { threshold },
        })
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
        Ok(Self {
            inner: RustChargingStrategy::Immediate { target_soc },
        })
    }

    #[staticmethod]
    fn nightly(
        off_peak_start_hour: f64,
        off_peak_end_hour: f64,
        target_soc: f64,
    ) -> PyResult<Self> {
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
        Ok(Self {
            inner: RustChargingStrategy::LowSoc {
                threshold,
                target_soc,
            },
        })
    }

    #[staticmethod]
    fn quick_then_wait(partial_soc: f64) -> PyResult<Self> {
        validate_soc("partial_soc", partial_soc)?;
        Ok(Self {
            inner: RustChargingStrategy::QuickThenWait { partial_soc },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target_soc, departure_schedule = None))]
    fn pre_departure(
        target_soc: f64,
        departure_schedule: Option<Vec<PyDepartureConstraint>>,
    ) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        Ok(Self {
            inner: RustChargingStrategy::PreDeparture {
                target_soc,
                departure_schedule: departure_schedule
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| d.inner)
                    .collect(),
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target_soc, departure_schedule = None, charge_buffer_hours = 2.0))]
    fn tou_aware(
        target_soc: f64,
        departure_schedule: Option<Vec<PyDepartureConstraint>>,
        charge_buffer_hours: f64,
    ) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        Ok(Self {
            inner: RustChargingStrategy::TouAware {
                target_soc,
                departure_schedule: departure_schedule
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| d.inner)
                    .collect(),
                charge_buffer_hours,
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
    #[pyo3(signature = (min_charge_rate_kw, departure_schedule = None))]
    fn solar_surplus(
        min_charge_rate_kw: f64,
        departure_schedule: Option<Vec<PyDepartureConstraint>>,
    ) -> PyResult<Self> {
        if !min_charge_rate_kw.is_finite() || min_charge_rate_kw < 0.0 {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "min_charge_rate_kw must be finite and >= 0, got {min_charge_rate_kw}"
            )));
        }
        Ok(Self {
            inner: RustChargingStrategy::SolarSurplus {
                min_charge_rate_kw,
                departure_schedule: departure_schedule
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| d.inner)
                    .collect(),
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
            RustChargingStrategy::Nightly {
                off_peak_start_hour,
                off_peak_end_hour,
                target_soc,
            } => {
                format!(
                    "Nightly(off_peak_start={off_peak_start_hour}, off_peak_end={off_peak_end_hour}, target_soc={target_soc})"
                )
            }
            RustChargingStrategy::LowSoc {
                threshold,
                target_soc,
            } => {
                format!("LowSoc(threshold={threshold}, target_soc={target_soc})")
            }
            RustChargingStrategy::QuickThenWait { partial_soc } => {
                format!("QuickThenWait(partial_soc={partial_soc})")
            }
            RustChargingStrategy::PreDeparture { target_soc, .. } => {
                format!("PreDeparture(target_soc={target_soc})")
            }
            RustChargingStrategy::TouAware {
                target_soc,
                charge_buffer_hours,
                ..
            } => {
                format!("TouAware(target_soc={target_soc}, buffer_hours={charge_buffer_hours})")
            }
            RustChargingStrategy::SolarSurplus {
                min_charge_rate_kw, ..
            } => {
                format!("SolarSurplus(min_charge_rate_kw={min_charge_rate_kw})")
            }
            RustChargingStrategy::V2H {
                discharge_threshold_soc,
                min_soc,
            } => {
                format!("V2H(discharge_threshold_soc={discharge_threshold_soc}, min_soc={min_soc})")
            }
            RustChargingStrategy::V2G {
                min_soc,
                max_export_kw,
                price_threshold,
            } => {
                format!(
                    "V2G(min_soc={min_soc}, max_export_kw={max_export_kw}, price_threshold={price_threshold})"
                )
            }
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

#[pyclass(name = "BatteryProductId", eq, hash, frozen, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyBatteryProductId {
    TeslaPw3,
    TeslaPw2,
    TeslaPw2Nca,
    TeslaPw3X2,
    EnphaseIq5p,
    EnphaseIq5pX2,
    EnphaseIq10c,
    FranklinApower,
    FranklinApower2,
    FranklinApower2X2,
    SolaredgeHome,
    LgResu10h,
}

#[pymethods]
impl PyBatteryProductId {
    #[staticmethod]
    fn tesla_pw3() -> Self {
        Self::TeslaPw3
    }
    #[staticmethod]
    fn tesla_pw2() -> Self {
        Self::TeslaPw2
    }
    #[staticmethod]
    fn tesla_pw2_nca() -> Self {
        Self::TeslaPw2Nca
    }
    #[staticmethod]
    fn tesla_pw3_x2() -> Self {
        Self::TeslaPw3X2
    }
    #[staticmethod]
    fn enphase_iq5p() -> Self {
        Self::EnphaseIq5p
    }
    #[staticmethod]
    fn enphase_iq5p_x2() -> Self {
        Self::EnphaseIq5pX2
    }
    #[staticmethod]
    fn enphase_iq10c() -> Self {
        Self::EnphaseIq10c
    }
    #[staticmethod]
    fn franklin_apower() -> Self {
        Self::FranklinApower
    }
    #[staticmethod]
    fn franklin_apower2() -> Self {
        Self::FranklinApower2
    }
    #[staticmethod]
    fn franklin_apower2_x2() -> Self {
        Self::FranklinApower2X2
    }
    #[staticmethod]
    fn solaredge_home() -> Self {
        Self::SolaredgeHome
    }
    #[staticmethod]
    fn lg_resu10h() -> Self {
        Self::LgResu10h
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustBatteryProductId>()
            .map(PyBatteryProductId::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    fn __repr__(&self) -> String {
        format!("BatteryProductId.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustBatteryProductId::from(*self).to_string()
    }
}

impl From<RustBatteryProductId> for PyBatteryProductId {
    fn from(id: RustBatteryProductId) -> Self {
        match id {
            RustBatteryProductId::TeslaPw3 => Self::TeslaPw3,
            RustBatteryProductId::TeslaPw2 => Self::TeslaPw2,
            RustBatteryProductId::TeslaPw2Nca => Self::TeslaPw2Nca,
            RustBatteryProductId::TeslaPw3X2 => Self::TeslaPw3X2,
            RustBatteryProductId::EnphaseIq5p => Self::EnphaseIq5p,
            RustBatteryProductId::EnphaseIq5pX2 => Self::EnphaseIq5pX2,
            RustBatteryProductId::EnphaseIq10c => Self::EnphaseIq10c,
            RustBatteryProductId::FranklinApower => Self::FranklinApower,
            RustBatteryProductId::FranklinApower2 => Self::FranklinApower2,
            RustBatteryProductId::FranklinApower2X2 => Self::FranklinApower2X2,
            RustBatteryProductId::SolaredgeHome => Self::SolaredgeHome,
            RustBatteryProductId::LgResu10h => Self::LgResu10h,
        }
    }
}

impl From<PyBatteryProductId> for RustBatteryProductId {
    fn from(id: PyBatteryProductId) -> Self {
        match id {
            PyBatteryProductId::TeslaPw3 => Self::TeslaPw3,
            PyBatteryProductId::TeslaPw2 => Self::TeslaPw2,
            PyBatteryProductId::TeslaPw2Nca => Self::TeslaPw2Nca,
            PyBatteryProductId::TeslaPw3X2 => Self::TeslaPw3X2,
            PyBatteryProductId::EnphaseIq5p => Self::EnphaseIq5p,
            PyBatteryProductId::EnphaseIq5pX2 => Self::EnphaseIq5pX2,
            PyBatteryProductId::EnphaseIq10c => Self::EnphaseIq10c,
            PyBatteryProductId::FranklinApower => Self::FranklinApower,
            PyBatteryProductId::FranklinApower2 => Self::FranklinApower2,
            PyBatteryProductId::FranklinApower2X2 => Self::FranklinApower2X2,
            PyBatteryProductId::SolaredgeHome => Self::SolaredgeHome,
            PyBatteryProductId::LgResu10h => Self::LgResu10h,
        }
    }
}

#[pyclass(name = "VehicleId", eq, hash, frozen, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyVehicleId {
    TeslaModelYLr,
    TeslaModelYSr,
    TeslaModel3Lr,
    ChevyBoltEv,
    ChevyBoltEuv,
    FordMacheSr,
    FordMacheEr,
    FordLightningEr,
    HyundaiIoniq5Lr,
    NissanLeaf30,
    Jeep4xe,
    ToyotaRav4Prime,
    ChevyVoltGen1,
}

#[pymethods]
impl PyVehicleId {
    #[staticmethod]
    fn tesla_model_y_lr() -> Self {
        Self::TeslaModelYLr
    }
    #[staticmethod]
    fn tesla_model_y_sr() -> Self {
        Self::TeslaModelYSr
    }
    #[staticmethod]
    fn tesla_model_3_lr() -> Self {
        Self::TeslaModel3Lr
    }
    #[staticmethod]
    fn chevy_bolt_ev() -> Self {
        Self::ChevyBoltEv
    }
    #[staticmethod]
    fn chevy_bolt_euv() -> Self {
        Self::ChevyBoltEuv
    }
    #[staticmethod]
    fn ford_mache_sr() -> Self {
        Self::FordMacheSr
    }
    #[staticmethod]
    fn ford_mache_er() -> Self {
        Self::FordMacheEr
    }
    #[staticmethod]
    fn ford_lightning_er() -> Self {
        Self::FordLightningEr
    }
    #[staticmethod]
    fn hyundai_ioniq5_lr() -> Self {
        Self::HyundaiIoniq5Lr
    }
    #[staticmethod]
    fn nissan_leaf30() -> Self {
        Self::NissanLeaf30
    }
    #[staticmethod]
    fn jeep_4xe() -> Self {
        Self::Jeep4xe
    }
    #[staticmethod]
    fn toyota_rav4_prime() -> Self {
        Self::ToyotaRav4Prime
    }
    #[staticmethod]
    fn chevy_volt_gen1() -> Self {
        Self::ChevyVoltGen1
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustVehicleId>()
            .map(PyVehicleId::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    fn __repr__(&self) -> String {
        format!("VehicleId.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustVehicleId::from(*self).to_string()
    }
}

impl From<RustVehicleId> for PyVehicleId {
    fn from(id: RustVehicleId) -> Self {
        match id {
            RustVehicleId::TeslaModelYLr => Self::TeslaModelYLr,
            RustVehicleId::TeslaModelYSr => Self::TeslaModelYSr,
            RustVehicleId::TeslaModel3Lr => Self::TeslaModel3Lr,
            RustVehicleId::ChevyBoltEv => Self::ChevyBoltEv,
            RustVehicleId::ChevyBoltEuv => Self::ChevyBoltEuv,
            RustVehicleId::FordMacheSr => Self::FordMacheSr,
            RustVehicleId::FordMacheEr => Self::FordMacheEr,
            RustVehicleId::FordLightningEr => Self::FordLightningEr,
            RustVehicleId::HyundaiIoniq5Lr => Self::HyundaiIoniq5Lr,
            RustVehicleId::NissanLeaf30 => Self::NissanLeaf30,
            RustVehicleId::Jeep4xe => Self::Jeep4xe,
            RustVehicleId::ToyotaRav4Prime => Self::ToyotaRav4Prime,
            RustVehicleId::ChevyVoltGen1 => Self::ChevyVoltGen1,
        }
    }
}

impl From<PyVehicleId> for RustVehicleId {
    fn from(id: PyVehicleId) -> Self {
        match id {
            PyVehicleId::TeslaModelYLr => Self::TeslaModelYLr,
            PyVehicleId::TeslaModelYSr => Self::TeslaModelYSr,
            PyVehicleId::TeslaModel3Lr => Self::TeslaModel3Lr,
            PyVehicleId::ChevyBoltEv => Self::ChevyBoltEv,
            PyVehicleId::ChevyBoltEuv => Self::ChevyBoltEuv,
            PyVehicleId::FordMacheSr => Self::FordMacheSr,
            PyVehicleId::FordMacheEr => Self::FordMacheEr,
            PyVehicleId::FordLightningEr => Self::FordLightningEr,
            PyVehicleId::HyundaiIoniq5Lr => Self::HyundaiIoniq5Lr,
            PyVehicleId::NissanLeaf30 => Self::NissanLeaf30,
            PyVehicleId::Jeep4xe => Self::Jeep4xe,
            PyVehicleId::ToyotaRav4Prime => Self::ToyotaRav4Prime,
            PyVehicleId::ChevyVoltGen1 => Self::ChevyVoltGen1,
        }
    }
}

#[pyclass(name = "EvArchetypeId", eq, hash, frozen, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyEvArchetypeId {
    DailyCommuterL2,
    DailyCommuterL1,
    LongCommuterL2,
    WfhOccasional,
    WfhL1Minimal,
    HeavyUseSuv,
    ShiftWorker,
    WeekendWarrior,
    WorkplaceCharger,
    RetireeL1,
    PhevCommuter,
    TouOptimizerCa,
}

#[pymethods]
impl PyEvArchetypeId {
    #[staticmethod]
    fn daily_commuter_l2() -> Self {
        Self::DailyCommuterL2
    }
    #[staticmethod]
    fn daily_commuter_l1() -> Self {
        Self::DailyCommuterL1
    }
    #[staticmethod]
    fn long_commuter_l2() -> Self {
        Self::LongCommuterL2
    }
    #[staticmethod]
    fn wfh_occasional() -> Self {
        Self::WfhOccasional
    }
    #[staticmethod]
    fn wfh_l1_minimal() -> Self {
        Self::WfhL1Minimal
    }
    #[staticmethod]
    fn heavy_use_suv() -> Self {
        Self::HeavyUseSuv
    }
    #[staticmethod]
    fn shift_worker() -> Self {
        Self::ShiftWorker
    }
    #[staticmethod]
    fn weekend_warrior() -> Self {
        Self::WeekendWarrior
    }
    #[staticmethod]
    fn workplace_charger() -> Self {
        Self::WorkplaceCharger
    }
    #[staticmethod]
    fn retiree_l1() -> Self {
        Self::RetireeL1
    }
    #[staticmethod]
    fn phev_commuter() -> Self {
        Self::PhevCommuter
    }
    #[staticmethod]
    fn tou_optimizer_ca() -> Self {
        Self::TouOptimizerCa
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        name.parse::<RustEvArchetypeId>()
            .map(PyEvArchetypeId::from)
            .map_err(PyErr::new::<pyo3::exceptions::PyValueError, _>)
    }

    #[staticmethod]
    fn archetype_catalog() -> Vec<PyEvArchetypeId> {
        RustEvArchetypeId::ALL
            .iter()
            .copied()
            .map(PyEvArchetypeId::from)
            .collect()
    }

    fn __repr__(&self) -> String {
        format!("EvArchetypeId.{:?}", self)
    }

    fn __str__(&self) -> String {
        RustEvArchetypeId::from(*self).to_string()
    }
}

impl From<RustEvArchetypeId> for PyEvArchetypeId {
    fn from(id: RustEvArchetypeId) -> Self {
        match id {
            RustEvArchetypeId::DailyCommuterL2 => Self::DailyCommuterL2,
            RustEvArchetypeId::DailyCommuterL1 => Self::DailyCommuterL1,
            RustEvArchetypeId::LongCommuterL2 => Self::LongCommuterL2,
            RustEvArchetypeId::WfhOccasional => Self::WfhOccasional,
            RustEvArchetypeId::WfhL1Minimal => Self::WfhL1Minimal,
            RustEvArchetypeId::HeavyUseSuv => Self::HeavyUseSuv,
            RustEvArchetypeId::ShiftWorker => Self::ShiftWorker,
            RustEvArchetypeId::WeekendWarrior => Self::WeekendWarrior,
            RustEvArchetypeId::WorkplaceCharger => Self::WorkplaceCharger,
            RustEvArchetypeId::RetireeL1 => Self::RetireeL1,
            RustEvArchetypeId::PhevCommuter => Self::PhevCommuter,
            RustEvArchetypeId::TouOptimizerCa => Self::TouOptimizerCa,
        }
    }
}

impl From<PyEvArchetypeId> for RustEvArchetypeId {
    fn from(id: PyEvArchetypeId) -> Self {
        match id {
            PyEvArchetypeId::DailyCommuterL2 => Self::DailyCommuterL2,
            PyEvArchetypeId::DailyCommuterL1 => Self::DailyCommuterL1,
            PyEvArchetypeId::LongCommuterL2 => Self::LongCommuterL2,
            PyEvArchetypeId::WfhOccasional => Self::WfhOccasional,
            PyEvArchetypeId::WfhL1Minimal => Self::WfhL1Minimal,
            PyEvArchetypeId::HeavyUseSuv => Self::HeavyUseSuv,
            PyEvArchetypeId::ShiftWorker => Self::ShiftWorker,
            PyEvArchetypeId::WeekendWarrior => Self::WeekendWarrior,
            PyEvArchetypeId::WorkplaceCharger => Self::WorkplaceCharger,
            PyEvArchetypeId::RetireeL1 => Self::RetireeL1,
            PyEvArchetypeId::PhevCommuter => Self::PhevCommuter,
            PyEvArchetypeId::TouOptimizerCa => Self::TouOptimizerCa,
        }
    }
}

// ---------------------------------------------------------------------------
// GridExportRule
// ---------------------------------------------------------------------------

#[pyclass(name = "GridExportRule", eq, hash, frozen, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyGridExportRule {
    SolarOnly,
    Unrestricted,
    Disabled,
}

#[pymethods]
impl PyGridExportRule {
    #[staticmethod]
    fn solar_only() -> Self {
        PyGridExportRule::SolarOnly
    }

    #[staticmethod]
    fn unrestricted() -> Self {
        PyGridExportRule::Unrestricted
    }

    #[staticmethod]
    fn disabled() -> Self {
        PyGridExportRule::Disabled
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "solar_only" | "solaronly" => Ok(PyGridExportRule::SolarOnly),
            "unrestricted" => Ok(PyGridExportRule::Unrestricted),
            "disabled" => Ok(PyGridExportRule::Disabled),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid GridExportRule variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("GridExportRule.{self:?}")
    }

    fn __str__(&self) -> String {
        match self {
            PyGridExportRule::SolarOnly => "SolarOnly".to_string(),
            PyGridExportRule::Unrestricted => "Unrestricted".to_string(),
            PyGridExportRule::Disabled => "Disabled".to_string(),
        }
    }
}

impl From<PyGridExportRule> for RustGridExportRule {
    fn from(v: PyGridExportRule) -> Self {
        match v {
            PyGridExportRule::SolarOnly => Self::SolarOnly,
            PyGridExportRule::Unrestricted => Self::Unrestricted,
            PyGridExportRule::Disabled => Self::Disabled,
        }
    }
}

impl From<RustGridExportRule> for PyGridExportRule {
    fn from(v: RustGridExportRule) -> Self {
        match v {
            RustGridExportRule::SolarOnly => Self::SolarOnly,
            RustGridExportRule::Unrestricted => Self::Unrestricted,
            RustGridExportRule::Disabled => Self::Disabled,
        }
    }
}

// ---------------------------------------------------------------------------
// StormWatchTrigger
// ---------------------------------------------------------------------------

#[pyclass(name = "StormWatchTrigger", eq, hash, frozen, from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PyStormWatchTrigger {
    #[pyo3(get)]
    pub is_manual: bool,
    #[pyo3(get)]
    pub wind_speed_threshold_m_s: f64,
    #[pyo3(get)]
    pub wind_speed_deactivation_threshold_m_s: f64,
}

impl Eq for PyStormWatchTrigger {}

impl std::hash::Hash for PyStormWatchTrigger {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.is_manual.hash(state);
        self.wind_speed_threshold_m_s.to_bits().hash(state);
        self.wind_speed_deactivation_threshold_m_s
            .to_bits()
            .hash(state);
    }
}

#[pymethods]
impl PyStormWatchTrigger {
    #[staticmethod]
    fn manual_enable() -> Self {
        Self {
            is_manual: true,
            wind_speed_threshold_m_s: 0.0,
            wind_speed_deactivation_threshold_m_s: 0.0,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (wind_speed_threshold_m_s = 25.0, wind_speed_deactivation_threshold_m_s = 0.0))]
    fn weather_signal(
        wind_speed_threshold_m_s: f64,
        wind_speed_deactivation_threshold_m_s: f64,
    ) -> Self {
        Self {
            is_manual: false,
            wind_speed_threshold_m_s,
            wind_speed_deactivation_threshold_m_s,
        }
    }

    fn __repr__(&self) -> String {
        if self.is_manual {
            "StormWatchTrigger.manual_enable()".to_string()
        } else {
            format!(
                "StormWatchTrigger.weather_signal(wind_speed_threshold_m_s={}, wind_speed_deactivation_threshold_m_s={})",
                self.wind_speed_threshold_m_s, self.wind_speed_deactivation_threshold_m_s
            )
        }
    }
}

impl From<PyStormWatchTrigger> for RustStormWatchTrigger {
    fn from(v: PyStormWatchTrigger) -> Self {
        if v.is_manual {
            Self::ManualEnable
        } else {
            Self::WeatherSignal {
                wind_speed_threshold_m_s: v.wind_speed_threshold_m_s,
                wind_speed_deactivation_threshold_m_s: v.wind_speed_deactivation_threshold_m_s,
            }
        }
    }
}

impl From<RustStormWatchTrigger> for PyStormWatchTrigger {
    fn from(v: RustStormWatchTrigger) -> Self {
        match v {
            RustStormWatchTrigger::ManualEnable => Self {
                is_manual: true,
                wind_speed_threshold_m_s: 0.0,
                wind_speed_deactivation_threshold_m_s: 0.0,
            },
            RustStormWatchTrigger::WeatherSignal {
                wind_speed_threshold_m_s,
                wind_speed_deactivation_threshold_m_s,
            } => Self {
                is_manual: false,
                wind_speed_threshold_m_s,
                wind_speed_deactivation_threshold_m_s,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// BmsAction
// ---------------------------------------------------------------------------

#[pyclass(name = "BmsAction", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub struct PyBmsAction {
    pub inner: RustBmsAction,
}

#[pymethods]
impl PyBmsAction {
    #[staticmethod]
    #[pyo3(signature = (rate_fraction = 1.0))]
    fn charge(rate_fraction: f64) -> PyResult<Self> {
        validate_soc("rate_fraction", rate_fraction)?;
        Ok(Self {
            inner: RustBmsAction::Charge { rate_fraction },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (rate_fraction = 1.0))]
    fn discharge(rate_fraction: f64) -> PyResult<Self> {
        validate_soc("rate_fraction", rate_fraction)?;
        Ok(Self {
            inner: RustBmsAction::Discharge { rate_fraction },
        })
    }

    #[staticmethod]
    fn idle() -> Self {
        Self {
            inner: RustBmsAction::Idle,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (target_soc = 0.5))]
    fn hold(target_soc: f64) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        Ok(Self {
            inner: RustBmsAction::Hold { target_soc },
        })
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            RustBmsAction::Charge { rate_fraction } => {
                format!("BmsAction.charge(rate_fraction={rate_fraction})")
            }
            RustBmsAction::Discharge { rate_fraction } => {
                format!("BmsAction.discharge(rate_fraction={rate_fraction})")
            }
            RustBmsAction::Idle => "BmsAction.idle()".to_string(),
            RustBmsAction::Hold { target_soc } => {
                format!("BmsAction.hold(target_soc={target_soc})")
            }
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ---------------------------------------------------------------------------
// BmsScheduleWindow
// ---------------------------------------------------------------------------

#[pyclass(name = "BmsScheduleWindow", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub struct PyBmsScheduleWindow {
    pub inner: RustBmsScheduleWindow,
}

fn parse_day_filter(s: &str) -> PyResult<DayFilter> {
    crate::utils::parse_day_filter(s)
}

fn day_filter_to_str(d: &DayFilter) -> &'static str {
    match d {
        DayFilter::Any => "any",
        DayFilter::Weekdays => "weekdays",
        DayFilter::Weekends => "weekends",
        DayFilter::Day(chrono::Weekday::Mon) => "monday",
        DayFilter::Day(chrono::Weekday::Tue) => "tuesday",
        DayFilter::Day(chrono::Weekday::Wed) => "wednesday",
        DayFilter::Day(chrono::Weekday::Thu) => "thursday",
        DayFilter::Day(chrono::Weekday::Fri) => "friday",
        DayFilter::Day(chrono::Weekday::Sat) => "saturday",
        DayFilter::Day(chrono::Weekday::Sun) => "sunday",
    }
}

#[pymethods]
impl PyBmsScheduleWindow {
    #[new]
    #[pyo3(signature = (day, start_minute, end_minute, action))]
    fn new(day: &str, start_minute: u16, end_minute: u16, action: PyBmsAction) -> PyResult<Self> {
        let day_filter = parse_day_filter(day)?;
        let tw = RustBmsTimeWindow {
            day: day_filter,
            start_minute,
            end_minute,
        };
        tw.validate()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        Ok(Self {
            inner: RustBmsScheduleWindow {
                time_window: tw,
                action: action.inner,
            },
        })
    }

    #[getter]
    fn day(&self) -> &'static str {
        day_filter_to_str(&self.inner.time_window.day)
    }

    #[getter]
    fn start_minute(&self) -> u16 {
        self.inner.time_window.start_minute
    }

    #[getter]
    fn end_minute(&self) -> u16 {
        self.inner.time_window.end_minute
    }

    #[getter]
    fn action(&self) -> PyBmsAction {
        PyBmsAction {
            inner: self.inner.action.clone(),
        }
    }

    fn __repr__(&self) -> String {
        let day = day_filter_to_str(&self.inner.time_window.day);
        let s = self.inner.time_window.start_minute;
        let e = self.inner.time_window.end_minute;
        let act = PyBmsAction {
            inner: self.inner.action.clone(),
        };
        format!(
            "BmsScheduleWindow(day={day:?}, start={s}, end={e}, action={})",
            act.__repr__()
        )
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ---------------------------------------------------------------------------
// DepartureConstraint
// ---------------------------------------------------------------------------

#[pyclass(name = "DepartureConstraint", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub struct PyDepartureConstraint {
    pub inner: RustDepartureConstraint,
}

#[pymethods]
impl PyDepartureConstraint {
    #[new]
    #[pyo3(signature = (day_filter = "weekdays", departure_minute = 480, target_soc = 0.8))]
    fn new(day_filter: &str, departure_minute: u32, target_soc: f64) -> PyResult<Self> {
        let day = parse_day_filter(day_filter)?;
        validate_soc("target_soc", target_soc)?;
        let dc = RustDepartureConstraint {
            day_filter: day,
            departure_minute,
            target_soc,
        };
        dc.validate()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        Ok(Self { inner: dc })
    }

    #[getter]
    fn day_filter(&self) -> &'static str {
        day_filter_to_str(&self.inner.day_filter)
    }

    #[getter]
    fn departure_minute(&self) -> u32 {
        self.inner.departure_minute
    }

    #[getter]
    fn target_soc(&self) -> f64 {
        self.inner.target_soc
    }

    fn __repr__(&self) -> String {
        let day = day_filter_to_str(&self.inner.day_filter);
        format!(
            "DepartureConstraint(day_filter={day:?}, departure_minute={}, target_soc={})",
            self.inner.departure_minute, self.inner.target_soc
        )
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ---------------------------------------------------------------------------
// BmsMode
// ---------------------------------------------------------------------------

#[pyclass(name = "BmsMode", from_py_object)]
#[derive(Clone, Debug, PartialEq)]
pub struct PyBmsMode {
    pub inner: RustBmsMode,
}

#[pymethods]
impl PyBmsMode {
    #[staticmethod]
    #[pyo3(signature = (min_soc = 0.1, max_soc = 1.0, solar_only_charging = false, surplus_deadband_kw = 0.0))]
    fn self_consumption(
        min_soc: f64,
        max_soc: f64,
        solar_only_charging: bool,
        surplus_deadband_kw: f64,
    ) -> PyResult<Self> {
        validate_soc("min_soc", min_soc)?;
        validate_soc("max_soc", max_soc)?;
        if min_soc > max_soc {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "min_soc must be <= max_soc",
            ));
        }
        Ok(Self {
            inner: RustBmsMode::SelfConsumption {
                min_soc,
                max_soc,
                solar_only_charging,
                surplus_deadband_kw,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (
        reserve_soc = 0.2,
        charge_threshold_percentile = 25.0,
        discharge_threshold_percentile = 75.0,
        solar_only_charging = false,
        price_deadband = 0.0,
        min_duration_steps = None,
    ))]
    fn time_of_use_optimization(
        reserve_soc: f64,
        charge_threshold_percentile: f64,
        discharge_threshold_percentile: f64,
        solar_only_charging: bool,
        price_deadband: f64,
        min_duration_steps: Option<usize>,
    ) -> PyResult<Self> {
        validate_soc("reserve_soc", reserve_soc)?;
        if !charge_threshold_percentile.is_finite()
            || !(0.0..=100.0).contains(&charge_threshold_percentile)
        {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "charge_threshold_percentile must be in [0, 100], got {charge_threshold_percentile}"
            )));
        }
        if !discharge_threshold_percentile.is_finite()
            || !(0.0..=100.0).contains(&discharge_threshold_percentile)
        {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "discharge_threshold_percentile must be in [0, 100], got {discharge_threshold_percentile}"
            )));
        }
        Ok(Self {
            inner: RustBmsMode::TimeOfUseOptimization {
                reserve_soc,
                charge_threshold_percentile,
                discharge_threshold_percentile,
                solar_only_charging,
                price_deadband,
                min_duration_steps,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target_soc = 0.8, charge_from_grid = true, charge_rate_fraction = 1.0, soc_deadband = 0.0))]
    fn backup_reserve(
        target_soc: f64,
        charge_from_grid: bool,
        charge_rate_fraction: f64,
        soc_deadband: f64,
    ) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        validate_soc("charge_rate_fraction", charge_rate_fraction)?;
        Ok(Self {
            inner: RustBmsMode::BackupReserve {
                target_soc,
                charge_from_grid,
                charge_rate_fraction,
                soc_deadband,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (base_mode, dr_discharge_rate = 1.0, min_soc_during_dr = 0.1, dr_deactivation_multiplier = 0.0, min_duration_steps = None))]
    fn demand_response(
        base_mode: PyBmsMode,
        dr_discharge_rate: f64,
        min_soc_during_dr: f64,
        dr_deactivation_multiplier: f64,
        min_duration_steps: Option<usize>,
    ) -> PyResult<Self> {
        validate_soc("dr_discharge_rate", dr_discharge_rate)?;
        validate_soc("min_soc_during_dr", min_soc_during_dr)?;
        Ok(Self {
            inner: RustBmsMode::DemandResponse {
                base_mode: Box::new(base_mode.inner),
                dr_discharge_rate,
                min_soc_during_dr,
                dr_deactivation_multiplier,
                min_duration_steps,
            },
        })
    }

    #[staticmethod]
    fn scheduled(windows: Vec<PyBmsScheduleWindow>) -> PyResult<Self> {
        if windows.is_empty() {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "scheduled windows must not be empty",
            ));
        }
        Ok(Self {
            inner: RustBmsMode::Scheduled {
                windows: windows.into_iter().map(|w| w.inner).collect(),
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target_soc = 1.0, trigger = "manual", base_mode = None, wind_speed_threshold_m_s = 25.0, wind_speed_deactivation_threshold_m_s = 0.0, min_duration_steps = None))]
    fn storm_watch(
        target_soc: f64,
        trigger: &str,
        base_mode: Option<PyBmsMode>,
        wind_speed_threshold_m_s: f64,
        wind_speed_deactivation_threshold_m_s: f64,
        min_duration_steps: Option<usize>,
    ) -> PyResult<Self> {
        validate_soc("target_soc", target_soc)?;
        let trigger = match trigger {
            "manual" => RustStormWatchTrigger::ManualEnable,
            "weather_signal" => RustStormWatchTrigger::WeatherSignal {
                wind_speed_threshold_m_s,
                wind_speed_deactivation_threshold_m_s,
            },
            other => {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "trigger must be 'manual' or 'weather_signal', got {other:?}"
                )));
            }
        };
        let base = base_mode.unwrap_or(PyBmsMode {
            inner: RustBmsMode::Manual,
        });
        Ok(Self {
            inner: RustBmsMode::StormWatch {
                target_soc,
                trigger,
                base_mode: Box::new(base.inner),
                min_duration_steps,
            },
        })
    }

    #[staticmethod]
    fn manual() -> Self {
        Self {
            inner: RustBmsMode::Manual,
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            RustBmsMode::SelfConsumption {
                min_soc,
                max_soc,
                solar_only_charging,
                surplus_deadband_kw,
            } => {
                let sc = if *solar_only_charging {
                    "True"
                } else {
                    "False"
                };
                format!(
                    "BmsMode.self_consumption(min_soc={min_soc}, max_soc={max_soc}, solar_only_charging={sc}, surplus_deadband_kw={surplus_deadband_kw})"
                )
            }
            RustBmsMode::TimeOfUseOptimization {
                reserve_soc,
                charge_threshold_percentile,
                discharge_threshold_percentile,
                solar_only_charging,
                price_deadband,
                ..
            } => {
                let sc = if *solar_only_charging {
                    "True"
                } else {
                    "False"
                };
                format!(
                    "BmsMode.time_of_use_optimization(reserve_soc={reserve_soc}, charge_threshold_percentile={charge_threshold_percentile}, discharge_threshold_percentile={discharge_threshold_percentile}, solar_only_charging={sc}, price_deadband={price_deadband})"
                )
            }
            RustBmsMode::BackupReserve {
                target_soc,
                charge_from_grid,
                charge_rate_fraction,
                soc_deadband,
            } => {
                let cfg = if *charge_from_grid { "True" } else { "False" };
                format!(
                    "BmsMode.backup_reserve(target_soc={target_soc}, charge_from_grid={cfg}, charge_rate_fraction={charge_rate_fraction}, soc_deadband={soc_deadband})"
                )
            }
            RustBmsMode::DemandResponse {
                base_mode,
                dr_discharge_rate,
                min_soc_during_dr,
                dr_deactivation_multiplier,
                ..
            } => {
                let base_repr = PyBmsMode {
                    inner: *base_mode.clone(),
                }
                .__repr__();
                format!(
                    "BmsMode.demand_response(base_mode={base_repr}, dr_discharge_rate={dr_discharge_rate}, min_soc_during_dr={min_soc_during_dr}, dr_deactivation_multiplier={dr_deactivation_multiplier})"
                )
            }
            RustBmsMode::Scheduled { windows } => {
                format!("BmsMode.scheduled(windows=[{} window(s)])", windows.len())
            }
            RustBmsMode::StormWatch {
                target_soc,
                trigger,
                base_mode,
                ..
            } => {
                let base_repr = PyBmsMode {
                    inner: *base_mode.clone(),
                }
                .__repr__();
                format!(
                    "BmsMode.storm_watch(target_soc={target_soc}, trigger={trigger:?}, base_mode={base_repr})"
                )
            }
            RustBmsMode::Manual => "BmsMode.manual()".to_string(),
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ── ExtrapolationStrategy ────────────────────────────────────────────

#[pyclass(name = "ExtrapolationStrategy", skip_from_py_object)]
#[derive(Clone, Copy)]
pub struct PyExtrapolationStrategy {
    inner: RustExtrapolationStrategy,
}

#[pymethods]
impl PyExtrapolationStrategy {
    #[classattr]
    const CLAMP: Self = Self {
        inner: RustExtrapolationStrategy::Clamp,
    };
    #[classattr]
    const NAN: Self = Self {
        inner: RustExtrapolationStrategy::NaN,
    };
    #[classattr]
    const LINEAR: Self = Self {
        inner: RustExtrapolationStrategy::Linear,
    };
    #[classattr]
    const NEAREST_NEIGHBOR: Self = Self {
        inner: RustExtrapolationStrategy::NearestNeighbor,
    };

    fn __repr__(&self) -> String {
        format!("ExtrapolationStrategy.{:?}", self.inner)
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
