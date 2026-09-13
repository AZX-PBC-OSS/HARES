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
    #[pyo3(signature = (kw, reactive_kvar=None, min_soc=None, max_soc=None))]
    /// Signed active power: positive charges, negative discharges. The Rust
    /// validator and every storage equipment treat the sign as the
    /// charge/discharge direction, so the binding rejects only non-finite
    /// values.
    pub fn power_setpoint(
        kw: f64,
        reactive_kvar: Option<f64>,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
    ) -> PyResult<Self> {
        validate_finite(kw, "active_power_kw")?;
        Ok(Self {
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: kw,
                reactive_power_kvar: reactive_kvar,
                min_soc,
                max_soc,
            },
        })
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
    pub fn ideal_capacity(capacity_w: f64) -> PyResult<Self> {
        validate_non_negative(capacity_w, "capacity_w")?;
        Ok(Self {
            signal: ControlSignal::IdealCapacity {
                capacity_w,
                degraded: false,
            },
        })
    }

    #[staticmethod]
    pub fn load_fraction(fraction: f64) -> PyResult<Self> {
        validate_range(fraction, 0.0, 1.0, "fraction")?;
        Ok(Self {
            signal: ControlSignal::LoadFraction { fraction },
        })
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
    pub fn soc_target(target: f64, min: Option<f64>, max: Option<f64>) -> PyResult<Self> {
        validate_range(target, 0.0, 1.0, "target_soc")?;
        if let Some(v) = min {
            validate_range(v, 0.0, 1.0, "min_soc")?;
        }
        if let Some(v) = max {
            validate_range(v, 0.0, 1.0, "max_soc")?;
        }
        Ok(Self {
            signal: ControlSignal::SOCTarget {
                target_soc: target,
                min_soc: min,
                max_soc: max,
            },
        })
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
    pub fn power_limit(max_power_kw: f64, ramp_rate_kw_per_s: Option<f64>) -> PyResult<Self> {
        validate_non_negative(max_power_kw, "max_power_kw")?;
        Ok(Self {
            signal: ControlSignal::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (on_fraction, period_s=None, component=None))]
    pub fn duty_cycle(
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<PyDutyCycleComponent>,
    ) -> PyResult<Self> {
        validate_range(on_fraction, 0.0, 1.0, "on_fraction")?;
        Ok(Self {
            signal: ControlSignal::DutyCycle {
                on_fraction,
                period_s,
                component: component.map(RustDutyCycleComponent::from),
            },
        })
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
    pub fn curtailment_percent(percent: f64) -> PyResult<Self> {
        validate_range(percent, 0.0, 100.0, "percent")?;
        Ok(Self {
            signal: ControlSignal::CurtailmentPercent { percent },
        })
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
    pub fn ev_drive(kwh: f64) -> PyResult<Self> {
        validate_non_negative(kwh, "kwh")?;
        Ok(Self {
            signal: ControlSignal::EvDrive { kwh },
        })
    }

    #[staticmethod]
    pub fn ev_away_charge(power_kw: f64) -> PyResult<Self> {
        validate_non_negative(power_kw, "power_kw")?;
        Ok(Self {
            signal: ControlSignal::EvAwayCharge { power_kw },
        })
    }

    #[staticmethod]
    pub fn ev_set_ready_by(departure_hour: f64, target_soc: f64) -> PyResult<Self> {
        validate_range(target_soc, 0.0, 1.0, "target_soc")?;
        Ok(Self {
            signal: ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            },
        })
    }

    #[staticmethod]
    #[pyo3(signature = (fraction))]
    pub fn max_capacity_fraction(fraction: f64) -> PyResult<Self> {
        validate_range(fraction, 0.0, 1.0, "fraction")?;
        Ok(Self {
            signal: ControlSignal::MaxCapacityFraction { fraction },
        })
    }

    #[staticmethod]
    pub fn event_delay(delay_s: f64) -> Self {
        Self {
            signal: ControlSignal::EventDelay { delay_s },
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
            "PowerSetpoint" => {
                let active_power_kw: f64 = dict_required(d, "active_power_kw")?;
                validate_finite(active_power_kw, "active_power_kw")?;
                ControlSignal::PowerSetpoint {
                    active_power_kw,
                    reactive_power_kvar: dict_optional(d, "reactive_power_kvar")?,
                    min_soc: dict_optional(d, "min_soc")?,
                    max_soc: dict_optional(d, "max_soc")?,
                }
            }
            "HumiditySetpoint" => ControlSignal::HumiditySetpoint {
                target_rh: dict_required(d, "target_rh")?,
                min_rh: dict_optional(d, "min_rh")?,
                max_rh: dict_optional(d, "max_rh")?,
            },
            "SelfConsumption" => ControlSignal::SelfConsumption {
                enabled: dict_required(d, "enabled")?,
                solar_only_charging: dict_required(d, "solar_only_charging")?,
            },
            "PowerLimit" => {
                let max_power_kw: f64 = dict_required(d, "max_power_kw")?;
                validate_non_negative(max_power_kw, "max_power_kw")?;
                ControlSignal::PowerLimit {
                    max_power_kw,
                    ramp_rate_kw_per_s: dict_optional(d, "ramp_rate_kw_per_s")?,
                }
            }
            "SOCTarget" => {
                let target_soc: f64 = dict_required(d, "target_soc")?;
                validate_range(target_soc, 0.0, 1.0, "target_soc")?;
                let min_soc: Option<f64> = dict_optional(d, "min_soc")?;
                if let Some(v) = min_soc {
                    validate_range(v, 0.0, 1.0, "min_soc")?;
                }
                let max_soc: Option<f64> = dict_optional(d, "max_soc")?;
                if let Some(v) = max_soc {
                    validate_range(v, 0.0, 1.0, "max_soc")?;
                }
                ControlSignal::SOCTarget {
                    target_soc,
                    min_soc,
                    max_soc,
                }
            }
            "DutyCycle" => {
                let on_fraction: f64 = dict_required(d, "on_fraction")?;
                validate_range(on_fraction, 0.0, 1.0, "on_fraction")?;
                ControlSignal::DutyCycle {
                    on_fraction,
                    period_s: dict_optional(d, "period_s")?,
                    component: parse_duty_cycle_component_or_enum(d, "component")?,
                }
            }
            "LoadFraction" => {
                let fraction: f64 = dict_required(d, "fraction")?;
                validate_range(fraction, 0.0, 1.0, "fraction")?;
                ControlSignal::LoadFraction { fraction }
            }
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
            "CurtailmentPercent" => {
                let percent: f64 = dict_required(d, "percent")?;
                validate_range(percent, 0.0, 100.0, "percent")?;
                ControlSignal::CurtailmentPercent { percent }
            }
            "ReactiveSetpoint" => ControlSignal::ReactiveSetpoint {
                kvar: dict_required(d, "kvar")?,
            },
            "PowerFactorSetpoint" => ControlSignal::PowerFactorSetpoint {
                power_factor: dict_required(d, "power_factor")?,
            },
            "InverterPriorityMode" => ControlSignal::InverterPriorityMode {
                priority: parse_inverter_priority_or_enum(d, "priority")?,
            },
            "IdealCapacity" => {
                let capacity_w: f64 = dict_required(d, "capacity_w")?;
                validate_non_negative(capacity_w, "capacity_w")?;
                ControlSignal::IdealCapacity {
                    capacity_w,
                    degraded: false,
                }
            }
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
            "EvDrive" => {
                let kwh: f64 = dict_required(d, "kwh")?;
                validate_non_negative(kwh, "kwh")?;
                ControlSignal::EvDrive { kwh }
            }
            "EvAwayCharge" => {
                let power_kw: f64 = dict_required(d, "power_kw")?;
                validate_non_negative(power_kw, "power_kw")?;
                ControlSignal::EvAwayCharge { power_kw }
            }
            "EvSetReadyBy" => {
                let target_soc: f64 = dict_required(d, "target_soc")?;
                validate_range(target_soc, 0.0, 1.0, "target_soc")?;
                ControlSignal::EvSetReadyBy {
                    departure_hour: dict_required(d, "departure_hour")?,
                    target_soc,
                }
            }
            "EventDelay" => ControlSignal::EventDelay {
                delay_s: dict_required(d, "delay_s")?,
            },
            "MaxCapacityFraction" => {
                let fraction: f64 = dict_required(d, "fraction")?;
                validate_range(fraction, 0.0, 1.0, "fraction")?;
                ControlSignal::MaxCapacityFraction { fraction }
            }
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
                min_soc,
                max_soc,
            } => {
                dict.set_item("type", "PowerSetpoint")?;
                dict.set_item("active_power_kw", active_power_kw)?;
                if let Some(kvar) = reactive_power_kvar {
                    dict.set_item("reactive_power_kvar", kvar)?;
                }
                if let Some(v) = min_soc {
                    dict.set_item("min_soc", v)?;
                }
                if let Some(v) = max_soc {
                    dict.set_item("max_soc", v)?;
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
            ControlSignal::IdealCapacity {
                capacity_w,
                degraded,
            } => {
                dict.set_item("type", "IdealCapacity")?;
                dict.set_item("capacity_w", capacity_w)?;
                dict.set_item("degraded", degraded)?;
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

fn validate_finite(value: f64, name: &str) -> PyResult<()> {
    if !value.is_finite() {
        return Err(PyValueError::new_err(format!(
            "{name} must be finite, got {value}"
        )));
    }
    Ok(())
}

fn validate_non_negative(value: f64, name: &str) -> PyResult<()> {
    if !value.is_finite() {
        return Err(PyValueError::new_err(format!(
            "{name} must be finite, got {value}"
        )));
    }
    if value < 0.0 {
        return Err(PyValueError::new_err(format!(
            "{name} must be >= 0, got {value}"
        )));
    }
    Ok(())
}

fn validate_range(value: f64, min: f64, max: f64, name: &str) -> PyResult<()> {
    if !value.is_finite() {
        return Err(PyValueError::new_err(format!(
            "{name} must be finite, got {value}"
        )));
    }
    if value < min || value > max {
        return Err(PyValueError::new_err(format!(
            "{name} must be in [{min}, {max}], got {value}"
        )));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_delay_constructs_correct_signal() {
        let sig = PyControlSignal::event_delay(30.0);
        assert!(matches!(
            sig.signal,
            ControlSignal::EventDelay { delay_s: 30.0 }
        ));
    }

    // --- constructor validation: non-negative ---

    #[test]
    fn power_setpoint_accepts_negative_kw_as_discharge() {
        assert!(PyControlSignal::power_setpoint(-1.0, None, None, None).is_ok());
    }

    #[test]
    fn power_setpoint_accepts_zero_and_positive() {
        assert!(PyControlSignal::power_setpoint(0.0, None, None, None).is_ok());
        assert!(PyControlSignal::power_setpoint(1.0, None, None, None).is_ok());
    }

    #[test]
    fn ideal_capacity_rejects_negative() {
        assert!(PyControlSignal::ideal_capacity(-1.0).is_err());
    }

    #[test]
    fn ideal_capacity_accepts_zero_and_positive() {
        assert!(PyControlSignal::ideal_capacity(0.0).is_ok());
        assert!(PyControlSignal::ideal_capacity(5000.0).is_ok());
    }

    #[test]
    fn power_limit_rejects_negative() {
        assert!(PyControlSignal::power_limit(-1.0, None).is_err());
    }

    #[test]
    fn power_limit_accepts_zero_and_positive() {
        assert!(PyControlSignal::power_limit(0.0, None).is_ok());
        assert!(PyControlSignal::power_limit(5.0, None).is_ok());
    }

    #[test]
    fn ev_drive_rejects_negative_kwh() {
        assert!(PyControlSignal::ev_drive(-1.0).is_err());
    }

    #[test]
    fn ev_drive_accepts_zero_and_positive() {
        assert!(PyControlSignal::ev_drive(0.0).is_ok());
        assert!(PyControlSignal::ev_drive(5.0).is_ok());
    }

    #[test]
    fn ev_away_charge_rejects_negative_power() {
        assert!(PyControlSignal::ev_away_charge(-1.0).is_err());
    }

    #[test]
    fn ev_away_charge_accepts_zero_and_positive() {
        assert!(PyControlSignal::ev_away_charge(0.0).is_ok());
        assert!(PyControlSignal::ev_away_charge(11.5).is_ok());
    }

    // --- constructor validation: fraction [0, 1] ---

    #[test]
    fn load_fraction_rejects_out_of_range() {
        assert!(PyControlSignal::load_fraction(-0.1).is_err());
        assert!(PyControlSignal::load_fraction(1.1).is_err());
    }

    #[test]
    fn load_fraction_accepts_boundaries() {
        assert!(PyControlSignal::load_fraction(0.0).is_ok());
        assert!(PyControlSignal::load_fraction(1.0).is_ok());
        assert!(PyControlSignal::load_fraction(0.75).is_ok());
    }

    #[test]
    fn duty_cycle_rejects_out_of_range_fraction() {
        assert!(PyControlSignal::duty_cycle(-0.1, None, None).is_err());
        assert!(PyControlSignal::duty_cycle(1.5, None, None).is_err());
    }

    #[test]
    fn duty_cycle_accepts_boundaries() {
        assert!(PyControlSignal::duty_cycle(0.0, None, None).is_ok());
        assert!(PyControlSignal::duty_cycle(1.0, None, None).is_ok());
        assert!(PyControlSignal::duty_cycle(0.5, Some(300.0), None).is_ok());
    }

    #[test]
    fn max_capacity_fraction_rejects_out_of_range() {
        assert!(PyControlSignal::max_capacity_fraction(-0.1).is_err());
        assert!(PyControlSignal::max_capacity_fraction(1.1).is_err());
    }

    #[test]
    fn max_capacity_fraction_accepts_boundaries() {
        assert!(PyControlSignal::max_capacity_fraction(0.0).is_ok());
        assert!(PyControlSignal::max_capacity_fraction(1.0).is_ok());
    }

    // --- constructor validation: SOC [0, 1] ---

    #[test]
    fn soc_target_rejects_out_of_range() {
        assert!(PyControlSignal::soc_target(-0.1, None, None).is_err());
        assert!(PyControlSignal::soc_target(1.1, None, None).is_err());
        assert!(PyControlSignal::soc_target(0.5, Some(-0.1), None).is_err());
        assert!(PyControlSignal::soc_target(0.5, None, Some(1.1)).is_err());
    }

    #[test]
    fn soc_target_accepts_boundaries() {
        assert!(PyControlSignal::soc_target(0.0, None, None).is_ok());
        assert!(PyControlSignal::soc_target(1.0, None, None).is_ok());
        assert!(PyControlSignal::soc_target(0.8, Some(0.2), Some(1.0)).is_ok());
    }

    #[test]
    fn ev_set_ready_by_rejects_out_of_range_soc() {
        assert!(PyControlSignal::ev_set_ready_by(7.0, -0.1).is_err());
        assert!(PyControlSignal::ev_set_ready_by(7.0, 1.1).is_err());
    }

    #[test]
    fn ev_set_ready_by_accepts_boundaries() {
        assert!(PyControlSignal::ev_set_ready_by(7.0, 0.0).is_ok());
        assert!(PyControlSignal::ev_set_ready_by(7.0, 1.0).is_ok());
        assert!(PyControlSignal::ev_set_ready_by(7.0, 0.85).is_ok());
    }

    // --- constructor validation: percent [0, 100] ---

    #[test]
    fn curtailment_percent_rejects_out_of_range() {
        assert!(PyControlSignal::curtailment_percent(-1.0).is_err());
        assert!(PyControlSignal::curtailment_percent(100.1).is_err());
    }

    #[test]
    fn curtailment_percent_accepts_boundaries() {
        assert!(PyControlSignal::curtailment_percent(0.0).is_ok());
        assert!(PyControlSignal::curtailment_percent(100.0).is_ok());
        assert!(PyControlSignal::curtailment_percent(50.0).is_ok());
    }

    // --- from_dict validation ---

    fn test_requires_python<F>(f: F)
    where
        F: for<'py> FnOnce(Python<'py>),
    {
        // The `auto-initialize` feature performs guarded, idempotent
        // interpreter startup on first attach. Calling `ffi::Py_Initialize()`
        // directly races that guard ("global import state already
        // initialized", then SIGSEGV from `assume_attached` without the GIL).
        Python::attach(f);
    }

    #[test]
    fn from_dict_accepts_negative_power_setpoint_as_discharge() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "PowerSetpoint").unwrap();
            d.set_item("active_power_kw", -3.0_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            let sig = PyControlSignal::from_dict(&cls, &d).unwrap();
            assert!(matches!(
                sig.signal,
                ControlSignal::PowerSetpoint {
                    active_power_kw: -3.0,
                    ..
                }
            ));
        });
    }

    #[test]
    fn from_dict_rejects_negative_max_power() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "PowerLimit").unwrap();
            d.set_item("max_power_kw", -1.0_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_out_of_range_soc() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "SOCTarget").unwrap();
            d.set_item("target_soc", 1.5_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_out_of_range_duty_cycle() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "DutyCycle").unwrap();
            d.set_item("on_fraction", 1.5_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_out_of_range_load_fraction() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "LoadFraction").unwrap();
            d.set_item("fraction", -0.5_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_out_of_range_curtailment() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "CurtailmentPercent").unwrap();
            d.set_item("percent", 150.0_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_negative_capacity_w() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "IdealCapacity").unwrap();
            d.set_item("capacity_w", -1.0_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_negative_ev_drive() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "EvDrive").unwrap();
            d.set_item("kwh", -1.0_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_negative_ev_away_charge() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "EvAwayCharge").unwrap();
            d.set_item("power_kw", -1.0_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_out_of_range_ev_target_soc() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "EvSetReadyBy").unwrap();
            d.set_item("departure_hour", 7.0_f64).unwrap();
            d.set_item("target_soc", 1.5_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_out_of_range_max_capacity_fraction() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "MaxCapacityFraction").unwrap();
            d.set_item("fraction", -0.5_f64).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_accepts_valid_boundary_values() {
        test_requires_python(|py| {
            let cls = py.get_type::<PyControlSignal>();
            let cases: [(&str, &[(&str, f64)]); 15] = [
                ("PowerSetpoint", &[("active_power_kw", 0.0)]),
                ("PowerLimit", &[("max_power_kw", 0.0)]),
                ("SOCTarget", &[("target_soc", 0.0)]),
                ("SOCTarget", &[("target_soc", 1.0)]),
                ("DutyCycle", &[("on_fraction", 0.0)]),
                ("DutyCycle", &[("on_fraction", 1.0)]),
                ("LoadFraction", &[("fraction", 0.0)]),
                ("LoadFraction", &[("fraction", 1.0)]),
                ("CurtailmentPercent", &[("percent", 0.0)]),
                ("CurtailmentPercent", &[("percent", 100.0)]),
                ("IdealCapacity", &[("capacity_w", 0.0)]),
                ("EvDrive", &[("kwh", 0.0)]),
                ("EvAwayCharge", &[("power_kw", 0.0)]),
                ("MaxCapacityFraction", &[("fraction", 0.0)]),
                ("MaxCapacityFraction", &[("fraction", 1.0)]),
            ];
            for (variant, fields) in cases {
                let d = PyDict::new(py);
                d.set_item("type", variant).unwrap();
                for &(k, v) in fields {
                    d.set_item(k, v).unwrap();
                }
                let result = PyControlSignal::from_dict(&cls, &d);
                assert!(
                    result.is_ok(),
                    "from_dict should accept {variant} with {fields:?}, got {result:?}"
                );
            }
        });
    }

    // --- NaN rejection: non-negative validators ---

    #[test]
    fn power_setpoint_rejects_nan() {
        assert!(PyControlSignal::power_setpoint(f64::NAN, None, None, None).is_err());
    }

    #[test]
    fn ideal_capacity_rejects_nan() {
        assert!(PyControlSignal::ideal_capacity(f64::NAN).is_err());
    }

    #[test]
    fn power_limit_rejects_nan() {
        assert!(PyControlSignal::power_limit(f64::NAN, None).is_err());
    }

    #[test]
    fn ev_drive_rejects_nan() {
        assert!(PyControlSignal::ev_drive(f64::NAN).is_err());
    }

    #[test]
    fn ev_away_charge_rejects_nan() {
        assert!(PyControlSignal::ev_away_charge(f64::NAN).is_err());
    }

    // --- NaN rejection: range validators [0, 1] ---

    #[test]
    fn load_fraction_rejects_nan() {
        assert!(PyControlSignal::load_fraction(f64::NAN).is_err());
    }

    #[test]
    fn duty_cycle_rejects_nan() {
        assert!(PyControlSignal::duty_cycle(f64::NAN, None, None).is_err());
    }

    #[test]
    fn soc_target_rejects_nan() {
        assert!(PyControlSignal::soc_target(f64::NAN, None, None).is_err());
        assert!(PyControlSignal::soc_target(0.5, Some(f64::NAN), None).is_err());
        assert!(PyControlSignal::soc_target(0.5, None, Some(f64::NAN)).is_err());
    }

    #[test]
    fn curtailment_percent_rejects_nan() {
        assert!(PyControlSignal::curtailment_percent(f64::NAN).is_err());
    }

    #[test]
    fn ev_set_ready_by_rejects_nan() {
        assert!(PyControlSignal::ev_set_ready_by(7.0, f64::NAN).is_err());
    }

    #[test]
    fn max_capacity_fraction_rejects_nan() {
        assert!(PyControlSignal::max_capacity_fraction(f64::NAN).is_err());
    }

    // --- NaN rejection: from_dict ---

    #[test]
    fn from_dict_rejects_nan_power_setpoint() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "PowerSetpoint").unwrap();
            d.set_item("active_power_kw", f64::NAN).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_nan_soc_target() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "SOCTarget").unwrap();
            d.set_item("target_soc", f64::NAN).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_nan_duty_cycle() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "DutyCycle").unwrap();
            d.set_item("on_fraction", f64::NAN).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_nan_ev_drive() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "EvDrive").unwrap();
            d.set_item("kwh", f64::NAN).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_nan_curtailment_percent() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "CurtailmentPercent").unwrap();
            d.set_item("percent", f64::NAN).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }

    #[test]
    fn from_dict_rejects_nan_ideal_capacity() {
        test_requires_python(|py| {
            let d = PyDict::new(py);
            d.set_item("type", "IdealCapacity").unwrap();
            d.set_item("capacity_w", f64::NAN).unwrap();
            let cls = py.get_type::<PyControlSignal>();
            assert!(PyControlSignal::from_dict(&cls, &d).is_err());
        });
    }
}
