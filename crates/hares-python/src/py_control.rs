//! Python bindings for control signals.

use hares_types::{
    ControlSignal, DRLevel, DutyCycleComponent as RustDutyCycleComponent,
    EvConnectionState as RustEvConnectionState, IdealCapacityMode, InverterPriority, OperatingMode,
    ProtocolId,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyType};

use crate::py_actor::{PyDRLevel, PyMode};
use crate::py_enums::{
    PyDutyCycleComponent, PyEvConnectionState, PyIdealCapacityMode, PyInverterPriority,
};

#[pyclass(name = "ControlSignal")]
#[derive(Debug)]
pub struct PyControlSignal {
    pub(crate) signal: ControlSignal,
}

#[pymethods]
impl PyControlSignal {
    #[staticmethod]
    #[pyo3(signature = (kw, reactive_kvar=None))]
    pub fn power_setpoint(kw: f64, reactive_kvar: Option<f64>) -> Self {
        Self {
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: kw,
                reactive_power_kvar: reactive_kvar,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (heat_c=None, cool_c=None, deadband_c=None))]
    pub fn thermal_setpoint(
        heat_c: Option<f64>,
        cool_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> Self {
        Self {
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: heat_c,
                cooling_setpoint_c: cool_c,
                deadband_c,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (heating_delta_c=None, cooling_delta_c=None))]
    pub fn thermal_setpoint_delta(
        heating_delta_c: Option<f64>,
        cooling_delta_c: Option<f64>,
    ) -> Self {
        Self {
            signal: ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            },
        }
    }

    #[staticmethod]
    pub fn ideal_capacity(capacity_w: f64) -> Self {
        Self {
            signal: ControlSignal::IdealCapacity { capacity_w },
        }
    }

    #[staticmethod]
    pub fn load_fraction(fraction: f64) -> Self {
        Self {
            signal: ControlSignal::LoadFraction { fraction },
        }
    }

    #[staticmethod]
    pub fn mode_override(mode: PyMode) -> Self {
        Self {
            signal: ControlSignal::ModeOverride {
                mode: mode.into_operating_mode(),
            },
        }
    }

    #[staticmethod]
    pub fn mode_override_str(mode: &str) -> PyResult<Self> {
        Ok(Self {
            signal: ControlSignal::ModeOverride {
                mode: parse_mode(mode)?,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (level, duration_s=None))]
    pub fn demand_response(level: PyDRLevel, duration_s: Option<f64>) -> Self {
        Self {
            signal: ControlSignal::DemandResponse {
                level: level.into_dr_level(),
                duration_s,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (level, duration_s=None))]
    pub fn demand_response_str(level: &str, duration_s: Option<f64>) -> PyResult<Self> {
        Ok(Self {
            signal: ControlSignal::DemandResponse {
                level: parse_dr_level(level)?,
                duration_s,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, min=None, max=None))]
    pub fn soc_target(target: f64, min: Option<f64>, max: Option<f64>) -> Self {
        Self {
            signal: ControlSignal::SOCTarget {
                target_soc: target,
                min_soc: min,
                max_soc: max,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (target_rh, min_rh=None, max_rh=None))]
    pub fn humidity_setpoint(target_rh: f64, min_rh: Option<f64>, max_rh: Option<f64>) -> Self {
        Self {
            signal: ControlSignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (max_power_kw, ramp_rate_kw_per_s=None))]
    pub fn power_limit(max_power_kw: f64, ramp_rate_kw_per_s: Option<f64>) -> Self {
        Self {
            signal: ControlSignal::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (on_fraction, period_s=None, component=None))]
    pub fn duty_cycle(
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<PyDutyCycleComponent>,
    ) -> Self {
        Self {
            signal: ControlSignal::DutyCycle {
                on_fraction,
                period_s,
                component: component.map(RustDutyCycleComponent::from),
            },
        }
    }

    #[staticmethod]
    pub fn grid_connect(connected: bool) -> Self {
        Self {
            signal: ControlSignal::GridConnect { connected },
        }
    }

    #[staticmethod]
    pub fn self_consumption(enabled: bool, solar_only_charging: bool) -> Self {
        Self {
            signal: ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            },
        }
    }

    #[staticmethod]
    pub fn curtailment_percent(percent: f64) -> Self {
        Self {
            signal: ControlSignal::CurtailmentPercent { percent },
        }
    }

    #[staticmethod]
    pub fn reactive_setpoint(kvar: f64) -> Self {
        Self {
            signal: ControlSignal::ReactiveSetpoint { kvar },
        }
    }

    #[staticmethod]
    pub fn power_factor_setpoint(power_factor: f64) -> Self {
        Self {
            signal: ControlSignal::PowerFactorSetpoint { power_factor },
        }
    }

    #[staticmethod]
    pub fn inverter_priority_mode(priority: PyInverterPriority) -> Self {
        Self {
            signal: ControlSignal::InverterPriorityMode {
                priority: priority.into(),
            },
        }
    }

    #[staticmethod]
    pub fn protocol_native(protocol_id: u16, payload: Vec<u8>) -> Self {
        Self {
            signal: ControlSignal::ProtocolNative {
                protocol: ProtocolId(protocol_id),
                payload,
            },
        }
    }

    #[staticmethod]
    pub fn ideal_capacity_mode_override(mode: PyIdealCapacityMode) -> Self {
        Self {
            signal: ControlSignal::IdealCapacityModeOverride { mode: mode.into() },
        }
    }

    #[staticmethod]
    pub fn ideal_capacity_mode_override_str(mode: &str) -> PyResult<Self> {
        let mode = match mode.to_lowercase().as_str() {
            "auto" => IdealCapacityMode::Auto,
            "on" => IdealCapacityMode::On,
            "off" => IdealCapacityMode::Off,
            _ => {
                return Err(PyValueError::new_err(format!(
                    "unsupported IdealCapacityMode `{mode}`"
                )));
            }
        };
        Ok(Self {
            signal: ControlSignal::IdealCapacityModeOverride { mode },
        })
    }

    #[staticmethod]
    pub fn ev_plug_in(state: PyEvConnectionState) -> Self {
        Self {
            signal: ControlSignal::EvPlugIn {
                state: RustEvConnectionState::from(state),
            },
        }
    }

    #[staticmethod]
    pub fn ev_drive(kwh: f64) -> Self {
        Self {
            signal: ControlSignal::EvDrive { kwh },
        }
    }

    #[staticmethod]
    pub fn ev_away_charge(power_kw: f64) -> Self {
        Self {
            signal: ControlSignal::EvAwayCharge { power_kw },
        }
    }

    #[staticmethod]
    pub fn ev_set_ready_by(departure_hour: f64, target_soc: f64) -> Self {
        Self {
            signal: ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            },
        }
    }

    #[staticmethod]
    #[pyo3(signature = (fraction))]
    pub fn max_capacity_fraction(fraction: f64) -> Self {
        Self {
            signal: ControlSignal::MaxCapacityFraction { fraction },
        }
    }

    #[classmethod]
    pub fn from_dict(_cls: &Bound<'_, PyType>, d: &Bound<'_, PyDict>) -> PyResult<Self> {
        let kind: String = dict_required(d, "type")?;
        let signal = match kind.as_str() {
            "ThermalSetpoint" => ControlSignal::ThermalSetpoint {
                heating_setpoint_c: dict_optional(d, "heating_setpoint_c")?,
                cooling_setpoint_c: dict_optional(d, "cooling_setpoint_c")?,
                deadband_c: dict_optional(d, "deadband_c")?,
            },
            "ModeOverride" => ControlSignal::ModeOverride {
                mode: parse_mode_or_enum(d, "mode")?,
            },
            "PowerSetpoint" => ControlSignal::PowerSetpoint {
                active_power_kw: dict_required(d, "active_power_kw")?,
                reactive_power_kvar: dict_optional(d, "reactive_power_kvar")?,
            },
            "HumiditySetpoint" => ControlSignal::HumiditySetpoint {
                target_rh: dict_required(d, "target_rh")?,
                min_rh: dict_optional(d, "min_rh")?,
                max_rh: dict_optional(d, "max_rh")?,
            },
            "SelfConsumption" => ControlSignal::SelfConsumption {
                enabled: dict_required(d, "enabled")?,
                solar_only_charging: dict_required(d, "solar_only_charging")?,
            },
            "PowerLimit" => ControlSignal::PowerLimit {
                max_power_kw: dict_required(d, "max_power_kw")?,
                ramp_rate_kw_per_s: dict_optional(d, "ramp_rate_kw_per_s")?,
            },
            "SOCTarget" => ControlSignal::SOCTarget {
                target_soc: dict_required(d, "target_soc")?,
                min_soc: dict_optional(d, "min_soc")?,
                max_soc: dict_optional(d, "max_soc")?,
            },
            "DutyCycle" => ControlSignal::DutyCycle {
                on_fraction: dict_required(d, "on_fraction")?,
                period_s: dict_optional(d, "period_s")?,
                component: parse_duty_cycle_component_or_enum(d, "component")?,
            },
            "LoadFraction" => ControlSignal::LoadFraction {
                fraction: dict_required(d, "fraction")?,
            },
            "GridConnect" => ControlSignal::GridConnect {
                connected: dict_required(d, "connected")?,
            },
            "DemandResponse" => ControlSignal::DemandResponse {
                level: parse_dr_level_or_enum(d, "level")?,
                duration_s: dict_optional(d, "duration_s")?,
            },
            "ProtocolNative" => ControlSignal::ProtocolNative {
                protocol: ProtocolId(dict_required(d, "protocol")?),
                payload: dict_required(d, "payload")?,
            },
            "CurtailmentPercent" => ControlSignal::CurtailmentPercent {
                percent: dict_required(d, "percent")?,
            },
            "ReactiveSetpoint" => ControlSignal::ReactiveSetpoint {
                kvar: dict_required(d, "kvar")?,
            },
            "PowerFactorSetpoint" => ControlSignal::PowerFactorSetpoint {
                power_factor: dict_required(d, "power_factor")?,
            },
            "InverterPriorityMode" => ControlSignal::InverterPriorityMode {
                priority: parse_inverter_priority_or_enum(d, "priority")?,
            },
            "IdealCapacity" => ControlSignal::IdealCapacity {
                capacity_w: dict_required(d, "capacity_w")?,
            },
            "ThermalSetpointDelta" => ControlSignal::ThermalSetpointDelta {
                heating_delta_c: dict_optional(d, "heating_delta_c")?,
                cooling_delta_c: dict_optional(d, "cooling_delta_c")?,
            },
            "IdealCapacityModeOverride" => {
                let mode_str: String = dict_required(d, "mode")?;
                let mode = match mode_str.to_lowercase().as_str() {
                    "auto" => IdealCapacityMode::Auto,
                    "on" => IdealCapacityMode::On,
                    "off" => IdealCapacityMode::Off,
                    _ => {
                        return Err(PyValueError::new_err(format!(
                            "unsupported IdealCapacityMode `{mode_str}`"
                        )));
                    }
                };
                ControlSignal::IdealCapacityModeOverride { mode }
            }
            "EvPlugIn" => {
                let state_str: String = dict_required(d, "state")?;
                let state = state_str
                    .parse::<hares_types::EvConnectionState>()
                    .map_err(PyValueError::new_err)?;
                ControlSignal::EvPlugIn { state }
            }
            "EvDrive" => ControlSignal::EvDrive {
                kwh: dict_required(d, "kwh")?,
            },
            "EvAwayCharge" => ControlSignal::EvAwayCharge {
                power_kw: dict_required(d, "power_kw")?,
            },
            "EvSetReadyBy" => ControlSignal::EvSetReadyBy {
                departure_hour: dict_required(d, "departure_hour")?,
                target_soc: dict_required(d, "target_soc")?,
            },
            "EventDelay" => ControlSignal::EventDelay {
                delay_s: dict_required(d, "delay_s")?,
            },
            "MaxCapacityFraction" => ControlSignal::MaxCapacityFraction {
                fraction: dict_required(d, "fraction")?,
            },
            _ => {
                return Err(PyValueError::new_err(format!(
                    "unsupported control signal type `{kind}`"
                )));
            }
        };

        Ok(Self { signal })
    }

    fn to_dict<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyDict>> {
        let dict = PyDict::new(py);
        match &self.signal {
            ControlSignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
            } => {
                dict.set_item("type", "PowerSetpoint")?;
                dict.set_item("active_power_kw", active_power_kw)?;
                if let Some(kvar) = reactive_power_kvar {
                    dict.set_item("reactive_power_kvar", kvar)?;
                }
            }
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                dict.set_item("type", "ThermalSetpoint")?;
                if let Some(v) = heating_setpoint_c {
                    dict.set_item("heating_setpoint_c", v)?;
                }
                if let Some(v) = cooling_setpoint_c {
                    dict.set_item("cooling_setpoint_c", v)?;
                }
                if let Some(v) = deadband_c {
                    dict.set_item("deadband_c", v)?;
                }
            }
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => {
                dict.set_item("type", "ThermalSetpointDelta")?;
                if let Some(v) = heating_delta_c {
                    dict.set_item("heating_delta_c", v)?;
                }
                if let Some(v) = cooling_delta_c {
                    dict.set_item("cooling_delta_c", v)?;
                }
            }
            ControlSignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            } => {
                dict.set_item("type", "HumiditySetpoint")?;
                dict.set_item("target_rh", target_rh)?;
                if let Some(v) = min_rh {
                    dict.set_item("min_rh", v)?;
                }
                if let Some(v) = max_rh {
                    dict.set_item("max_rh", v)?;
                }
            }
            ControlSignal::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            } => {
                dict.set_item("type", "PowerLimit")?;
                dict.set_item("max_power_kw", max_power_kw)?;
                if let Some(v) = ramp_rate_kw_per_s {
                    dict.set_item("ramp_rate_kw_per_s", v)?;
                }
            }
            ControlSignal::DutyCycle {
                on_fraction,
                period_s,
                component,
            } => {
                dict.set_item("type", "DutyCycle")?;
                dict.set_item("on_fraction", on_fraction)?;
                if let Some(v) = period_s {
                    dict.set_item("period_s", v)?;
                }
                if let Some(c) = component {
                    let c_str = match c {
                        RustDutyCycleComponent::Compressor => "Compressor",
                        RustDutyCycleComponent::BackupElement => "BackupElement",
                    };
                    dict.set_item("component", c_str)?;
                }
            }
            ControlSignal::GridConnect { connected } => {
                dict.set_item("type", "GridConnect")?;
                dict.set_item("connected", connected)?;
            }
            ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            } => {
                dict.set_item("type", "SelfConsumption")?;
                dict.set_item("enabled", enabled)?;
                dict.set_item("solar_only_charging", solar_only_charging)?;
            }
            ControlSignal::CurtailmentPercent { percent } => {
                dict.set_item("type", "CurtailmentPercent")?;
                dict.set_item("percent", percent)?;
            }
            ControlSignal::ReactiveSetpoint { kvar } => {
                dict.set_item("type", "ReactiveSetpoint")?;
                dict.set_item("kvar", kvar)?;
            }
            ControlSignal::PowerFactorSetpoint { power_factor } => {
                dict.set_item("type", "PowerFactorSetpoint")?;
                dict.set_item("power_factor", power_factor)?;
            }
            ControlSignal::InverterPriorityMode { priority } => {
                dict.set_item("type", "InverterPriorityMode")?;
                let p_str = match priority {
                    InverterPriority::Watt => "Watt",
                    InverterPriority::Var => "Var",
                    InverterPriority::Cpf => "Cpf",
                };
                dict.set_item("priority", p_str)?;
            }
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => {
                dict.set_item("type", "SOCTarget")?;
                dict.set_item("target_soc", target_soc)?;
                if let Some(v) = min_soc {
                    dict.set_item("min_soc", v)?;
                }
                if let Some(v) = max_soc {
                    dict.set_item("max_soc", v)?;
                }
            }
            ControlSignal::LoadFraction { fraction } => {
                dict.set_item("type", "LoadFraction")?;
                dict.set_item("fraction", fraction)?;
            }
            ControlSignal::ModeOverride { mode } => {
                dict.set_item("type", "ModeOverride")?;
                let m_str = match mode {
                    OperatingMode::Off => "Off",
                    OperatingMode::Heating => "Heating",
                    OperatingMode::Cooling => "Cooling",
                    OperatingMode::Defrost => "Defrost",
                    OperatingMode::Standby => "Standby",
                    OperatingMode::Charging => "Charging",
                    OperatingMode::Discharging => "Discharging",
                    OperatingMode::HeatingHP => "HeatingHP",
                    OperatingMode::HeatingER => "HeatingER",
                    OperatingMode::HeatingHPAndER => "HeatingHPAndER",
                    OperatingMode::HeatPumpWH => "HeatPumpWH",
                    OperatingMode::BackupElement => "BackupElement",
                    OperatingMode::On => "On",
                };
                dict.set_item("mode", m_str)?;
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                dict.set_item("type", "DemandResponse")?;
                let l_str = match level {
                    DRLevel::Normal => "Normal",
                    DRLevel::Moderate => "Moderate",
                    DRLevel::High => "High",
                    DRLevel::Critical => "Critical",
                    DRLevel::GridEmergency => "GridEmergency",
                };
                dict.set_item("level", l_str)?;
                if let Some(v) = duration_s {
                    dict.set_item("duration_s", v)?;
                }
            }
            ControlSignal::ProtocolNative { protocol, payload } => {
                dict.set_item("type", "ProtocolNative")?;
                dict.set_item("protocol", protocol.0)?;
                dict.set_item("payload", PyBytes::new(py, payload))?;
            }
            ControlSignal::IdealCapacity { capacity_w } => {
                dict.set_item("type", "IdealCapacity")?;
                dict.set_item("capacity_w", capacity_w)?;
            }
            ControlSignal::IdealCapacityModeOverride { mode } => {
                dict.set_item("type", "IdealCapacityModeOverride")?;
                let m_str = match mode {
                    IdealCapacityMode::Auto => "Auto",
                    IdealCapacityMode::On => "On",
                    IdealCapacityMode::Off => "Off",
                };
                dict.set_item("mode", m_str)?;
            }
            ControlSignal::EvPlugIn { state } => {
                dict.set_item("type", "EvPlugIn")?;
                dict.set_item("state", format!("{state}"))?;
            }
            ControlSignal::EvDrive { kwh } => {
                dict.set_item("type", "EvDrive")?;
                dict.set_item("kwh", kwh)?;
            }
            ControlSignal::EvAwayCharge { power_kw } => {
                dict.set_item("type", "EvAwayCharge")?;
                dict.set_item("power_kw", power_kw)?;
            }
            ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => {
                dict.set_item("type", "EvSetReadyBy")?;
                dict.set_item("departure_hour", departure_hour)?;
                dict.set_item("target_soc", target_soc)?;
            }
            ControlSignal::EventDelay { delay_s } => {
                dict.set_item("type", "EventDelay")?;
                dict.set_item("delay_s", delay_s)?;
            }
            ControlSignal::MaxCapacityFraction { fraction } => {
                dict.set_item("type", "MaxCapacityFraction")?;
                dict.set_item("fraction", fraction)?;
            }
        }
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!("ControlSignal({:?})", self.signal)
    }
}

fn dict_required<T>(d: &Bound<'_, PyDict>, key: &str) -> PyResult<T>
where
    T: for<'a, 'py> FromPyObject<'a, 'py>,
{
    let Some(value) = d.get_item(key)? else {
        return Err(PyValueError::new_err(format!(
            "missing required key `{key}`"
        )));
    };
    value
        .extract::<T>()
        .map_err(|_| PyValueError::new_err(format!("invalid value for `{key}`")))
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

fn parse_mode(mode: &str) -> PyResult<OperatingMode> {
    match mode {
        "Off" => Ok(OperatingMode::Off),
        "Heating" => Ok(OperatingMode::Heating),
        "Cooling" => Ok(OperatingMode::Cooling),
        "Defrost" => Ok(OperatingMode::Defrost),
        "Standby" => Ok(OperatingMode::Standby),
        "Charging" => Ok(OperatingMode::Charging),
        "Discharging" => Ok(OperatingMode::Discharging),
        "HeatingHP" => Ok(OperatingMode::HeatingHP),
        "HeatingER" => Ok(OperatingMode::HeatingER),
        "HeatingHPAndER" => Ok(OperatingMode::HeatingHPAndER),
        "HeatPumpWH" => Ok(OperatingMode::HeatPumpWH),
        "BackupElement" => Ok(OperatingMode::BackupElement),
        "On" => Ok(OperatingMode::On),
        _ => Err(PyValueError::new_err(format!(
            "unsupported OperatingMode `{mode}`"
        ))),
    }
}

fn parse_dr_level(level: &str) -> PyResult<DRLevel> {
    match level {
        "Normal" => Ok(DRLevel::Normal),
        "Moderate" => Ok(DRLevel::Moderate),
        "High" => Ok(DRLevel::High),
        "Critical" => Ok(DRLevel::Critical),
        "GridEmergency" => Ok(DRLevel::GridEmergency),
        _ => Err(PyValueError::new_err(format!(
            "unsupported DRLevel `{level}`"
        ))),
    }
}

fn parse_inverter_priority(priority: &str) -> PyResult<InverterPriority> {
    match priority {
        "Watt" => Ok(InverterPriority::Watt),
        "Var" => Ok(InverterPriority::Var),
        "Cpf" => Ok(InverterPriority::Cpf),
        _ => Err(PyValueError::new_err(format!(
            "unsupported InverterPriority `{priority}`"
        ))),
    }
}

fn parse_mode_or_enum(d: &Bound<'_, PyDict>, key: &str) -> PyResult<OperatingMode> {
    if let Some(value) = d.get_item(key)? {
        if let Ok(mode) = value.extract::<PyMode>() {
            return Ok(mode.into_operating_mode());
        }
        if let Ok(s) = value.extract::<String>() {
            return parse_mode(&s);
        }
    }
    Err(PyValueError::new_err(format!("invalid value for `{key}`")))
}

fn parse_dr_level_or_enum(d: &Bound<'_, PyDict>, key: &str) -> PyResult<DRLevel> {
    if let Some(value) = d.get_item(key)? {
        if let Ok(level) = value.extract::<PyDRLevel>() {
            return Ok(level.into_dr_level());
        }
        if let Ok(s) = value.extract::<String>() {
            return parse_dr_level(&s);
        }
    }
    Err(PyValueError::new_err(format!("invalid value for `{key}`")))
}

fn parse_inverter_priority_or_enum(d: &Bound<'_, PyDict>, key: &str) -> PyResult<InverterPriority> {
    if let Some(value) = d.get_item(key)? {
        if let Ok(priority) = value.extract::<PyInverterPriority>() {
            return Ok(priority.into());
        }
        if let Ok(s) = value.extract::<String>() {
            return parse_inverter_priority(&s);
        }
    }
    Err(PyValueError::new_err(format!("invalid value for `{key}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_capacity_fraction_constructs_correct_signal() {
        let sig = PyControlSignal::max_capacity_fraction(0.5);
        assert!(matches!(
            sig.signal,
            ControlSignal::MaxCapacityFraction { fraction: 0.5 }
        ));
    }
}

fn parse_duty_cycle_component_or_enum(
    d: &Bound<'_, PyDict>,
    key: &str,
) -> PyResult<Option<RustDutyCycleComponent>> {
    if let Some(value) = d.get_item(key)? {
        if value.is_none() {
            return Ok(None);
        }
        if let Ok(component) = value.extract::<PyDutyCycleComponent>() {
            return Ok(Some(component.into()));
        }
        if let Ok(s) = value.extract::<String>() {
            return Ok(match s.to_lowercase().as_str() {
                "compressor" | "hp" => Some(RustDutyCycleComponent::Compressor),
                "backupelement" | "backup" | "er" => Some(RustDutyCycleComponent::BackupElement),
                _ => {
                    return Err(PyValueError::new_err(format!(
                        "unsupported DutyCycleComponent `{s}`"
                    )));
                }
            });
        }
    }
    Ok(None)
}
