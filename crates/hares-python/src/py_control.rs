//! Python bindings for control signals.

use hares_types::{ControlSignal, DRLevel, InverterPriority, OperatingMode, ProtocolId};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};

#[pyclass(name = "ControlSignal")]
#[derive(Debug)]
pub struct PyControlSignal {
    pub(crate) signal: ControlSignal,
}

#[pymethods]
impl PyControlSignal {
    #[staticmethod]
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
                mode: parse_mode(&dict_required::<String>(d, "mode")?)?,
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
            "DutyCycle" => {
                let component_str: Option<String> = dict_optional(d, "component")?;
                let component = component_str.as_deref().and_then(|s| match s {
                    "Compressor" | "compressor" | "hp" => {
                        Some(hares_types::DutyCycleComponent::Compressor)
                    }
                    "BackupElement" | "backup" | "er" => {
                        Some(hares_types::DutyCycleComponent::BackupElement)
                    }
                    _ => None,
                });
                ControlSignal::DutyCycle {
                    on_fraction: dict_required(d, "on_fraction")?,
                    period_s: dict_optional(d, "period_s")?,
                    component,
                }
            }
            "LoadFraction" => ControlSignal::LoadFraction {
                fraction: dict_required(d, "fraction")?,
            },
            "GridConnect" => ControlSignal::GridConnect {
                connected: dict_required(d, "connected")?,
            },
            "DemandResponse" => ControlSignal::DemandResponse {
                level: parse_dr_level(&dict_required::<String>(d, "level")?)?,
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
                priority: parse_inverter_priority(&dict_required::<String>(d, "priority")?)?,
            },
            _ => {
                return Err(PyValueError::new_err(format!(
                    "unsupported control signal type `{kind}`"
                )));
            }
        };

        Ok(Self { signal })
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
