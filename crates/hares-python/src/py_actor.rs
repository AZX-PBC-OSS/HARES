//! Python bindings for actors.
//!
//! Provides Python actors that implement the Rust [`Actor`] trait.
//! Python users subclass the `Actor` base class and implement `decide(env)`.
//!
//! # Strong Types
//!
//! All signal types are strongly typed enums to prevent runtime errors:
//! - [`Mode`]: Off, Heating, Cooling, Standby
//! - [`Priority`]: Schedule, UserOverride, Grid, Safety
//! - [`Signal`]: ThermalSetpoint, ModeOverride, LoadFraction, PowerLimit
//!
//! # Example (Python)
//!
//! ```python
//! from hares import Actor, DispatchRequest, Signal, Mode, Priority
//!
//! class MyThermostat(Actor):
//!     def __init__(self, target: str):
//!         self.target = target
//!         self.name = f"Thermostat({target})"
//!
//!     def decide(self, env) -> list[DispatchRequest]:
//!         zones = env["zones"]
//!         if zones and zones[0]["temperature_c"] < 18.0:
//!             return [DispatchRequest.thermal_setpoint(
//!                 target=self.target,
//!                 heating_c=20.0,
//!                 priority=Priority.user_override(),
//!             )]
//!         return []
//! ```

use std::sync::Arc;

use hares_control::{DispatchRequest, DispatchTarget, PriorityTier};
use hares_types::{
    ControlSignal, DutyCycleComponent, EnvironmentState, EvConnectionState, OperatingMode,
    ProtocolId,
};

use crate::py_enums::{
    PyDutyCycleComponent, PyEvConnectionState, PyIdealCapacityMode, PyInverterPriority,
};
use pyo3::prelude::*;
use pyo3::types::PyDict;

/// Python actor base class.
#[pyclass(name = "Actor", subclass)]
pub struct PyActor {
    cached_name: Arc<str>,
}

#[pymethods]
impl PyActor {
    #[new]
    fn new() -> Self {
        Self {
            cached_name: Arc::from("PyActor"),
        }
    }

    #[setter]
    fn set_name(&mut self, name: String) {
        self.cached_name = Arc::from(name);
    }

    #[getter]
    fn name(&self) -> &str {
        &self.cached_name
    }
}

/// Rust wrapper for Python actor that implements the Actor trait.
pub struct PyActorWrapper {
    obj: Py<PyActor>,
    name: Arc<str>,
    /// Set when `decide()` raises an exception or returns an invalid type.
    /// Once set, `healthy()` returns false and the engine should not dispatch
    /// output from this actor.
    pub last_error: Option<String>,
}

impl PyActorWrapper {
    pub fn new(py: Python<'_>, obj: Py<PyActor>) -> Self {
        let name = obj
            .bind(py)
            .getattr("name")
            .and_then(|n| n.extract::<String>())
            .unwrap_or_else(|_| "PyActor".to_string());
        Self {
            name: Arc::from(name),
            obj,
            last_error: None,
        }
    }
}

impl hares_core::Actor for PyActorWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn healthy(&self) -> bool {
        self.last_error.is_none()
    }

    fn decide(&mut self, env: &EnvironmentState, out: &mut Vec<DispatchRequest>) {
        Python::attach(|py| {
            let obj = self.obj.bind(py);

            // Refresh cached name in case Python code updated it
            if let Ok(n) = obj.getattr("name").and_then(|v| v.extract::<String>()) {
                if n.as_str() != &*self.name {
                    self.name = Arc::from(n);
                }
            }

            let env_dict = environment_to_py_dict(py, env);

            let result = obj.call_method1("decide", (&env_dict,));
            match result {
                Ok(result) => match result.extract::<Vec<PyDispatchRequest>>() {
                    Ok(requests) => {
                        for req in requests {
                            out.push(req.into_dispatch_request());
                        }
                    }
                    Err(e) => {
                        let msg = format!("decide() returned invalid type: {e}");
                        tracing::error!(
                            actor = %self.name,
                            error = %e,
                            "Python actor decide() returned invalid type"
                        );
                        self.last_error = Some(msg);
                    }
                },
                Err(e) => {
                    let msg = format!("decide() raised exception: {e}");
                    tracing::error!(
                        actor = %self.name,
                        error = %e,
                        "Python actor decide() raised exception"
                    );
                    self.last_error = Some(msg);
                }
            }
        });

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                Python::try_attach(|py| py.import("sys").is_ok()).unwrap_or(false),
                "Python interpreter unreachable after PyActorWrapper::decide() — \
                 Python init state may be corrupted"
            );
        }
    }
}

fn environment_to_py_dict<'py>(py: Python<'py>, env: &EnvironmentState) -> Bound<'py, PyDict> {
    let dict = PyDict::new(py);

    let zones: Vec<Bound<'py, PyDict>> = env
        .zones
        .iter()
        .map(|zone| {
            let z = PyDict::new(py);
            let _ = z.set_item("id", zone.id.0);
            let _ = z.set_item("temperature_c", zone.temperature_c);
            let _ = z.set_item("humidity_ratio", zone.humidity_ratio);
            let _ = z.set_item(
                "relative_humidity",
                hares_physics::psychrometrics::zone_relative_humidity(
                    zone,
                    env.weather.pressure_pa(),
                ),
            );
            let _ = z.set_item("volume_m3", zone.volume_m3);
            z
        })
        .collect();
    let _ = dict.set_item("zones", zones);

    let weather = PyDict::new(py);
    let _ = weather.set_item("outdoor_temp_c", env.weather.outdoor_temp_c);
    let _ = weather.set_item("outdoor_humidity_ratio", env.weather.outdoor_humidity_ratio);
    let _ = weather.set_item("wind_speed_m_s", env.weather.wind_speed_m_s);
    let _ = weather.set_item("wind_dir_deg", env.weather.wind_dir_deg);
    let _ = weather.set_item("ground_temp_c", env.weather.ground_temp_c);
    let _ = weather.set_item("sky_temp_c", env.weather.sky_temp_c);
    let _ = weather.set_item("pressure_kpa", env.weather.pressure_kpa);
    let _ = weather.set_item("ghi_w_m2", env.weather.ghi_w_m2);
    let _ = weather.set_item("dni_w_m2", env.weather.dni_w_m2);
    let _ = weather.set_item("dhi_w_m2", env.weather.dhi_w_m2);
    let _ = weather.set_item("solar_altitude_deg", env.weather.solar_altitude_deg);
    let _ = dict.set_item("weather", weather);

    let grid = PyDict::new(py);
    let _ = grid.set_item("voltage_pu", env.grid.voltage_pu);
    let _ = grid.set_item("frequency_hz", env.grid.frequency_hz);
    // Outage/islanding observability (VPP / resilience RL): `voltage_pu` is
    // the utility service voltage (0.0 = outage); `bus_voltage_pu` is what
    // equipment actually sees (nonzero when a backup source islands the
    // home). See GridState in hares-types.
    let _ = grid.set_item("bus_voltage_pu", env.grid.bus_voltage_pu());
    let _ = grid.set_item("islanded", env.grid.islanded());
    let _ = dict.set_item("grid", grid);

    let _ = dict.set_item("current_time", env.current_time.to_rfc3339());
    let _ = dict.set_item("time_res_s", env.time_res.num_seconds());

    dict
}

/// Python dispatch request with strongly typed fields.
#[pyclass(name = "DispatchRequest", from_py_object)]
#[derive(Clone)]
pub struct PyDispatchRequest {
    #[pyo3(get)]
    pub target: String,
    #[pyo3(get)]
    pub signal: PySignal,
    #[pyo3(get)]
    pub priority: PyPriority,
}

/// Strongly typed signal variants.
#[pyclass(name = "Signal", from_py_object)]
#[derive(Clone, Debug)]
pub enum PySignal {
    ThermalSetpoint {
        heating_c: Option<f64>,
        cooling_c: Option<f64>,
        deadband_c: Option<f64>,
    },
    ThermalSetpointDelta {
        heating_delta_c: Option<f64>,
        cooling_delta_c: Option<f64>,
    },
    ModeOverride {
        mode: PyMode,
    },
    LoadFraction {
        fraction: f64,
    },
    PowerLimit {
        max_kw: f64,
    },
    PowerSetpoint {
        active_power_kw: f64,
        reactive_power_kvar: Option<f64>,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
    },
    SOCTarget {
        target_soc: f64,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
    },
    DutyCycle {
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<PyDutyCycleComponent>,
    },
    DemandResponse {
        level: PyDRLevel,
        duration_s: Option<f64>,
    },
    IdealCapacity {
        capacity_w: f64,
        degraded: bool,
    },
    EvPlugIn {
        state: PyEvConnectionState,
    },
    EvDrive {
        kwh: f64,
    },
    EvSetReadyBy {
        departure_hour: f64,
        target_soc: f64,
    },
    EvAwayCharge {
        power_kw: f64,
    },
    EventDelay {
        delay_s: f64,
    },
    HumiditySetpoint {
        target_rh: f64,
        min_rh: Option<f64>,
        max_rh: Option<f64>,
    },
    GridConnect {
        connected: bool,
    },
    SelfConsumption {
        enabled: bool,
        solar_only_charging: bool,
    },
    ProtocolNative {
        protocol_id: u16,
        payload: Vec<u8>,
    },
    CurtailmentPercent {
        percent: f64,
    },
    ReactiveSetpoint {
        kvar: f64,
    },
    PowerFactorSetpoint {
        power_factor: f64,
    },
    InverterPriorityMode {
        priority: PyInverterPriority,
    },
    IdealCapacityModeOverride {
        mode: PyIdealCapacityMode,
    },
    MaxCapacityFraction {
        fraction: f64,
    },
}

/// Strongly typed DR levels.
#[pyclass(name = "DRLevel", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyDRLevel {
    Normal,
    Moderate,
    High,
    Critical,
    GridEmergency,
}

/// Strongly typed operating modes.
#[pyclass(name = "Mode", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PyMode {
    Off,
    Heating,
    Cooling,
    Standby,
    Defrost,
    Charging,
    Discharging,
    HeatingHP,
    HeatingER,
    HeatingHPAndER,
    HeatPumpWH,
    BackupElement,
    On,
}

/// Strongly typed priority tiers.
#[pyclass(name = "Priority", from_py_object)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyPriority {
    Schedule,
    UserOverride,
    Grid,
    Safety,
}

impl PyDispatchRequest {
    pub fn into_dispatch_request(self) -> DispatchRequest {
        DispatchRequest {
            target: DispatchTarget::ByName(Arc::from(self.target)),
            signal: self.signal.into_control_signal(),
            priority: self.priority.into_priority_tier(),
        }
    }
}

impl PySignal {
    pub fn into_control_signal(self) -> ControlSignal {
        let result = match self {
            PySignal::ThermalSetpoint {
                heating_c,
                cooling_c,
                deadband_c,
            } => ControlSignal::ThermalSetpoint {
                heating_setpoint_c: heating_c,
                cooling_setpoint_c: cooling_c,
                deadband_c,
            },
            PySignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            },
            PySignal::ModeOverride { mode } => ControlSignal::ModeOverride {
                mode: mode.into_operating_mode(),
            },
            PySignal::LoadFraction { fraction } => ControlSignal::LoadFraction { fraction },
            PySignal::PowerLimit { max_kw } => ControlSignal::PowerLimit {
                max_power_kw: max_kw,
                ramp_rate_kw_per_s: None,
            },
            PySignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
                min_soc,
                max_soc,
            } => ControlSignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
                min_soc,
                max_soc,
            },
            PySignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            } => ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            },
            PySignal::DutyCycle {
                on_fraction,
                period_s,
                component,
            } => ControlSignal::DutyCycle {
                on_fraction,
                period_s,
                component: component.map(DutyCycleComponent::from),
            },
            PySignal::DemandResponse { level, duration_s } => ControlSignal::DemandResponse {
                level: level.into_dr_level(),
                duration_s,
            },
            PySignal::IdealCapacity {
                capacity_w,
                degraded,
            } => ControlSignal::IdealCapacity {
                capacity_w,
                degraded,
            },
            PySignal::EvPlugIn { state } => ControlSignal::EvPlugIn {
                state: EvConnectionState::from(state),
            },
            PySignal::EvDrive { kwh } => ControlSignal::EvDrive { kwh },
            PySignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            } => ControlSignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            },
            PySignal::EvAwayCharge { power_kw } => ControlSignal::EvAwayCharge { power_kw },
            PySignal::EventDelay { delay_s } => ControlSignal::EventDelay { delay_s },
            PySignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            } => ControlSignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            },
            PySignal::GridConnect { connected } => ControlSignal::GridConnect { connected },
            PySignal::SelfConsumption {
                enabled,
                solar_only_charging,
            } => ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            },
            PySignal::ProtocolNative {
                protocol_id,
                payload,
            } => ControlSignal::ProtocolNative {
                protocol: ProtocolId(protocol_id),
                payload,
            },
            PySignal::CurtailmentPercent { percent } => {
                ControlSignal::CurtailmentPercent { percent }
            }
            PySignal::ReactiveSetpoint { kvar } => ControlSignal::ReactiveSetpoint { kvar },
            PySignal::PowerFactorSetpoint { power_factor } => {
                ControlSignal::PowerFactorSetpoint { power_factor }
            }
            PySignal::InverterPriorityMode { priority } => ControlSignal::InverterPriorityMode {
                priority: priority.into(),
            },
            PySignal::IdealCapacityModeOverride { mode } => {
                ControlSignal::IdealCapacityModeOverride { mode: mode.into() }
            }
            PySignal::MaxCapacityFraction { fraction } => {
                ControlSignal::MaxCapacityFraction { fraction }
            }
        };
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert_ne!(
                result.required_capability().bits(),
                0,
                "PySignal variant mapped to zero-capability ControlSignal"
            );
            if let ControlSignal::DutyCycle {
                component: Some(_),
                on_fraction,
                ..
            } = &result
            {
                debug_assert!(
                    on_fraction.is_finite() && (0.0..=1.0).contains(on_fraction),
                    "DutyCycle with component field has invalid on_fraction: {on_fraction}"
                );
            }
        }
        #[cfg(feature = "observe")]
        {
            dispatch_observer::record_dispatch(&result);
            if let ControlSignal::DutyCycle {
                component: Some(comp),
                on_fraction,
                ..
            } = &result
            {
                dispatch_observer::record_duty_cycle_component(*comp, *on_fraction);
            }
        }
        result
    }
}

#[cfg(feature = "observe")]
mod dispatch_observer {
    use std::sync::{LazyLock, Mutex};

    use hares_types::{ControlSignal, DutyCycleComponent};

    static COUNTERS: LazyLock<Mutex<[u64; 25]>> = LazyLock::new(|| Mutex::new([0; 25]));
    static DUTY_CYCLE_COMPONENT_EVENTS: LazyLock<Mutex<Vec<(DutyCycleComponent, f64)>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));

    fn variant_index(signal: &ControlSignal) -> usize {
        match signal {
            ControlSignal::ThermalSetpoint { .. } => 0,
            ControlSignal::HumiditySetpoint { .. } => 1,
            ControlSignal::PowerSetpoint { .. } => 2,
            ControlSignal::PowerLimit { .. } => 3,
            ControlSignal::SOCTarget { .. } => 4,
            ControlSignal::ModeOverride { .. } => 5,
            ControlSignal::DutyCycle { .. } => 6,
            ControlSignal::LoadFraction { .. } => 7,
            ControlSignal::GridConnect { .. } => 8,
            ControlSignal::SelfConsumption { .. } => 9,
            ControlSignal::DemandResponse { .. } => 10,
            ControlSignal::ProtocolNative { .. } => 11,
            ControlSignal::CurtailmentPercent { .. } => 12,
            ControlSignal::ReactiveSetpoint { .. } => 13,
            ControlSignal::PowerFactorSetpoint { .. } => 14,
            ControlSignal::InverterPriorityMode { .. } => 15,
            ControlSignal::IdealCapacity { .. } => 16,
            ControlSignal::ThermalSetpointDelta { .. } => 17,
            ControlSignal::IdealCapacityModeOverride { .. } => 18,
            ControlSignal::EvPlugIn { .. } => 19,
            ControlSignal::EvDrive { .. } => 20,
            ControlSignal::EvAwayCharge { .. } => 21,
            ControlSignal::EvSetReadyBy { .. } => 22,
            ControlSignal::EventDelay { .. } => 23,
            ControlSignal::MaxCapacityFraction { .. } => 24,
        }
    }

    pub fn record_dispatch(signal: &ControlSignal) {
        let idx = variant_index(signal);
        if let Ok(mut counters) = COUNTERS.lock() {
            counters[idx] += 1;
        }
    }

    pub fn record_duty_cycle_component(component: DutyCycleComponent, on_fraction: f64) {
        if let Ok(mut events) = DUTY_CYCLE_COMPONENT_EVENTS.lock() {
            events.push((component, on_fraction));
        }
    }
}

impl PyDRLevel {
    pub fn into_dr_level(self) -> hares_types::DRLevel {
        match self {
            PyDRLevel::Normal => hares_types::DRLevel::Normal,
            PyDRLevel::Moderate => hares_types::DRLevel::Moderate,
            PyDRLevel::High => hares_types::DRLevel::High,
            PyDRLevel::Critical => hares_types::DRLevel::Critical,
            PyDRLevel::GridEmergency => hares_types::DRLevel::GridEmergency,
        }
    }
}

impl PyMode {
    pub fn into_operating_mode(self) -> OperatingMode {
        match self {
            PyMode::Off => OperatingMode::Off,
            PyMode::Heating => OperatingMode::Heating,
            PyMode::Cooling => OperatingMode::Cooling,
            PyMode::Standby => OperatingMode::Standby,
            PyMode::Defrost => OperatingMode::Defrost,
            PyMode::Charging => OperatingMode::Charging,
            PyMode::Discharging => OperatingMode::Discharging,
            PyMode::HeatingHP => OperatingMode::HeatingHP,
            PyMode::HeatingER => OperatingMode::HeatingER,
            PyMode::HeatingHPAndER => OperatingMode::HeatingHPAndER,
            PyMode::HeatPumpWH => OperatingMode::HeatPumpWH,
            PyMode::BackupElement => OperatingMode::BackupElement,
            PyMode::On => OperatingMode::On,
        }
    }
}

impl PyPriority {
    pub fn into_priority_tier(self) -> PriorityTier {
        match self {
            PyPriority::Schedule => PriorityTier::Schedule,
            PyPriority::UserOverride => PriorityTier::UserOverride,
            PyPriority::Grid => PriorityTier::Grid,
            PyPriority::Safety => PriorityTier::Safety,
        }
    }
}

#[pymethods]
impl PyDispatchRequest {
    #[staticmethod]
    #[pyo3(signature = (target, heating_c=None, cooling_c=None, deadband_c=None, priority=None))]
    fn thermal_setpoint(
        target: String,
        heating_c: Option<f64>,
        cooling_c: Option<f64>,
        deadband_c: Option<f64>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::ThermalSetpoint {
                heating_c,
                cooling_c,
                deadband_c,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, heating_delta_c=None, cooling_delta_c=None, priority=None))]
    fn thermal_setpoint_delta(
        target: String,
        heating_delta_c: Option<f64>,
        cooling_delta_c: Option<f64>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, mode, priority=None))]
    fn mode_override(target: String, mode: PyMode, priority: Option<PyPriority>) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::ModeOverride { mode },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, fraction, priority=None))]
    fn load_fraction(
        target: String,
        fraction: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::LoadFraction { fraction },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, max_kw, priority=None))]
    fn power_limit(target: String, max_kw: f64, priority: Option<PyPriority>) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::PowerLimit { max_kw },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, active_power_kw, reactive_power_kvar=None, min_soc=None, max_soc=None, priority=None))]
    fn power_setpoint(
        target: String,
        active_power_kw: f64,
        reactive_power_kvar: Option<f64>,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
                min_soc,
                max_soc,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, target_soc, min_soc=None, max_soc=None, priority=None))]
    fn soc_target(
        target: String,
        target_soc: f64,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, on_fraction, period_s=None, component=None, priority=None))]
    fn duty_cycle(
        target: String,
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<PyDutyCycleComponent>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::DutyCycle {
                on_fraction,
                period_s,
                component,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, level, duration_s=None, priority=None))]
    fn demand_response(
        target: String,
        level: PyDRLevel,
        duration_s: Option<f64>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::DemandResponse { level, duration_s },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, capacity_w, priority=None))]
    fn ideal_capacity(
        target: String,
        capacity_w: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::IdealCapacity {
                capacity_w,
                degraded: false,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, state, priority=None))]
    fn ev_plug_in(
        target: String,
        state: PyEvConnectionState,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::EvPlugIn { state },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, kwh, priority=None))]
    fn ev_drive(target: String, kwh: f64, priority: Option<PyPriority>) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::EvDrive { kwh },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, power_kw, priority=None))]
    fn ev_away_charge(
        target: String,
        power_kw: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::EvAwayCharge { power_kw },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, departure_hour, target_soc, priority=None))]
    fn ev_set_ready_by(
        target: String,
        departure_hour: f64,
        target_soc: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::EvSetReadyBy {
                departure_hour,
                target_soc,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, delay_s, priority=None))]
    fn event_delay(target: String, delay_s: f64, priority: Option<PyPriority>) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::EventDelay { delay_s },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, target_rh, min_rh=None, max_rh=None, priority=None))]
    fn humidity_setpoint(
        target: String,
        target_rh: f64,
        min_rh: Option<f64>,
        max_rh: Option<f64>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, connected, priority=None))]
    fn grid_connect(
        target: String,
        connected: bool,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::GridConnect { connected },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, enabled, solar_only_charging, priority=None))]
    fn self_consumption(
        target: String,
        enabled: bool,
        solar_only_charging: bool,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::SelfConsumption {
                enabled,
                solar_only_charging,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, protocol_id, payload, priority=None))]
    fn protocol_native(
        target: String,
        protocol_id: u16,
        payload: Vec<u8>,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::ProtocolNative {
                protocol_id,
                payload,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, percent, priority=None))]
    fn curtailment_percent(
        target: String,
        percent: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::CurtailmentPercent { percent },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, kvar, priority=None))]
    fn reactive_setpoint(
        target: String,
        kvar: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::ReactiveSetpoint { kvar },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, power_factor, priority=None))]
    fn power_factor_setpoint(
        target: String,
        power_factor: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::PowerFactorSetpoint { power_factor },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, priority_enum, priority=None))]
    fn inverter_priority_mode(
        target: String,
        priority_enum: PyInverterPriority,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::InverterPriorityMode {
                priority: priority_enum,
            },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, mode, priority=None))]
    fn ideal_capacity_mode_override(
        target: String,
        mode: PyIdealCapacityMode,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::IdealCapacityModeOverride { mode },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (target, fraction, priority=None))]
    fn max_capacity_fraction(
        target: String,
        fraction: f64,
        priority: Option<PyPriority>,
    ) -> PyResult<Self> {
        Ok(Self {
            target,
            signal: PySignal::MaxCapacityFraction { fraction },
            priority: priority.unwrap_or(PyPriority::Schedule),
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "DispatchRequest(target={:?}, signal={:?}, priority={:?})",
            self.target, self.signal, self.priority
        )
    }
}

#[pymethods]
impl PySignal {
    #[staticmethod]
    #[pyo3(signature = (heating_c=None, cooling_c=None, deadband_c=None))]
    fn thermal_setpoint(
        heating_c: Option<f64>,
        cooling_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> Self {
        PySignal::ThermalSetpoint {
            heating_c,
            cooling_c,
            deadband_c,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (heating_delta_c=None, cooling_delta_c=None))]
    fn thermal_setpoint_delta(heating_delta_c: Option<f64>, cooling_delta_c: Option<f64>) -> Self {
        PySignal::ThermalSetpointDelta {
            heating_delta_c,
            cooling_delta_c,
        }
    }

    #[staticmethod]
    fn mode_override(mode: PyMode) -> Self {
        PySignal::ModeOverride { mode }
    }

    #[staticmethod]
    fn load_fraction(fraction: f64) -> Self {
        PySignal::LoadFraction { fraction }
    }

    #[staticmethod]
    fn power_limit(max_kw: f64) -> Self {
        PySignal::PowerLimit { max_kw }
    }

    #[staticmethod]
    #[pyo3(signature = (active_power_kw, reactive_power_kvar=None, min_soc=None, max_soc=None))]
    fn power_setpoint(
        active_power_kw: f64,
        reactive_power_kvar: Option<f64>,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
    ) -> Self {
        PySignal::PowerSetpoint {
            active_power_kw,
            reactive_power_kvar,
            min_soc,
            max_soc,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (target_soc, min_soc=None, max_soc=None))]
    fn soc_target(target_soc: f64, min_soc: Option<f64>, max_soc: Option<f64>) -> Self {
        PySignal::SOCTarget {
            target_soc,
            min_soc,
            max_soc,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (on_fraction, period_s=None, component=None))]
    fn duty_cycle(
        on_fraction: f64,
        period_s: Option<f64>,
        component: Option<PyDutyCycleComponent>,
    ) -> Self {
        PySignal::DutyCycle {
            on_fraction,
            period_s,
            component,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (level, duration_s=None))]
    fn demand_response(level: PyDRLevel, duration_s: Option<f64>) -> Self {
        PySignal::DemandResponse { level, duration_s }
    }

    #[staticmethod]
    fn ideal_capacity(capacity_w: f64) -> Self {
        PySignal::IdealCapacity {
            capacity_w,
            degraded: false,
        }
    }

    #[staticmethod]
    fn ev_plug_in(state: PyEvConnectionState) -> Self {
        PySignal::EvPlugIn { state }
    }

    #[staticmethod]
    fn ev_drive(kwh: f64) -> Self {
        PySignal::EvDrive { kwh }
    }

    #[staticmethod]
    fn ev_set_ready_by(departure_hour: f64, target_soc: f64) -> Self {
        PySignal::EvSetReadyBy {
            departure_hour,
            target_soc,
        }
    }

    #[staticmethod]
    fn ev_away_charge(power_kw: f64) -> Self {
        PySignal::EvAwayCharge { power_kw }
    }

    #[staticmethod]
    fn event_delay(delay_s: f64) -> Self {
        PySignal::EventDelay { delay_s }
    }

    #[staticmethod]
    #[pyo3(signature = (target_rh, min_rh=None, max_rh=None))]
    fn humidity_setpoint(target_rh: f64, min_rh: Option<f64>, max_rh: Option<f64>) -> Self {
        PySignal::HumiditySetpoint {
            target_rh,
            min_rh,
            max_rh,
        }
    }

    #[staticmethod]
    fn grid_connect(connected: bool) -> Self {
        PySignal::GridConnect { connected }
    }

    #[staticmethod]
    fn self_consumption(enabled: bool, solar_only_charging: bool) -> Self {
        PySignal::SelfConsumption {
            enabled,
            solar_only_charging,
        }
    }

    #[staticmethod]
    fn protocol_native(protocol_id: u16, payload: Vec<u8>) -> Self {
        PySignal::ProtocolNative {
            protocol_id,
            payload,
        }
    }

    #[staticmethod]
    fn curtailment_percent(percent: f64) -> Self {
        PySignal::CurtailmentPercent { percent }
    }

    #[staticmethod]
    fn reactive_setpoint(kvar: f64) -> Self {
        PySignal::ReactiveSetpoint { kvar }
    }

    #[staticmethod]
    fn power_factor_setpoint(power_factor: f64) -> Self {
        PySignal::PowerFactorSetpoint { power_factor }
    }

    #[staticmethod]
    fn inverter_priority_mode(priority: PyInverterPriority) -> Self {
        PySignal::InverterPriorityMode { priority }
    }

    #[staticmethod]
    fn ideal_capacity_mode_override(mode: PyIdealCapacityMode) -> PyResult<Self> {
        Ok(PySignal::IdealCapacityModeOverride { mode })
    }

    #[staticmethod]
    fn max_capacity_fraction(fraction: f64) -> Self {
        PySignal::MaxCapacityFraction { fraction }
    }

    fn __repr__(&self) -> String {
        format!("{:?}", self)
    }
}

#[pymethods]
impl PyMode {
    #[staticmethod]
    fn off() -> Self {
        PyMode::Off
    }

    #[staticmethod]
    fn heating() -> Self {
        PyMode::Heating
    }

    #[staticmethod]
    fn cooling() -> Self {
        PyMode::Cooling
    }

    #[staticmethod]
    fn standby() -> Self {
        PyMode::Standby
    }

    #[staticmethod]
    fn defrost() -> Self {
        PyMode::Defrost
    }

    #[staticmethod]
    fn charging() -> Self {
        PyMode::Charging
    }

    #[staticmethod]
    fn discharging() -> Self {
        PyMode::Discharging
    }

    #[staticmethod]
    fn heating_hp() -> Self {
        PyMode::HeatingHP
    }

    #[staticmethod]
    fn heating_er() -> Self {
        PyMode::HeatingER
    }

    #[staticmethod]
    fn heating_hp_and_er() -> Self {
        PyMode::HeatingHPAndER
    }

    #[staticmethod]
    fn heat_pump_wh() -> Self {
        PyMode::HeatPumpWH
    }

    #[staticmethod]
    fn backup_element() -> Self {
        PyMode::BackupElement
    }

    #[staticmethod]
    fn on() -> Self {
        PyMode::On
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "off" => Ok(PyMode::Off),
            "heating" => Ok(PyMode::Heating),
            "cooling" => Ok(PyMode::Cooling),
            "standby" => Ok(PyMode::Standby),
            "defrost" => Ok(PyMode::Defrost),
            "charging" => Ok(PyMode::Charging),
            "discharging" => Ok(PyMode::Discharging),
            "heating_hp" => Ok(PyMode::HeatingHP),
            "heating_er" => Ok(PyMode::HeatingER),
            "heating_hp_and_er" => Ok(PyMode::HeatingHPAndER),
            "heat_pump_wh" => Ok(PyMode::HeatPumpWH),
            "backup_element" => Ok(PyMode::BackupElement),
            "on" => Ok(PyMode::On),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid OperatingMode variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyMode::Off => "off".to_string(),
            PyMode::Heating => "heating".to_string(),
            PyMode::Cooling => "cooling".to_string(),
            PyMode::Standby => "standby".to_string(),
            PyMode::Defrost => "defrost".to_string(),
            PyMode::Charging => "charging".to_string(),
            PyMode::Discharging => "discharging".to_string(),
            PyMode::HeatingHP => "heating_hp".to_string(),
            PyMode::HeatingER => "heating_er".to_string(),
            PyMode::HeatingHPAndER => "heating_hp_and_er".to_string(),
            PyMode::HeatPumpWH => "heat_pump_wh".to_string(),
            PyMode::BackupElement => "backup_element".to_string(),
            PyMode::On => "on".to_string(),
        }
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other_mode) = other.extract::<PyMode>() {
            Ok(*self == other_mode)
        } else {
            Ok(false)
        }
    }

    fn __hash__(&self) -> u64 {
        match self {
            PyMode::Off => 0,
            PyMode::Heating => 1,
            PyMode::Cooling => 2,
            PyMode::Standby => 3,
            PyMode::Defrost => 4,
            PyMode::Charging => 5,
            PyMode::Discharging => 6,
            PyMode::HeatingHP => 7,
            PyMode::HeatingER => 8,
            PyMode::HeatingHPAndER => 9,
            PyMode::HeatPumpWH => 10,
            PyMode::BackupElement => 11,
            PyMode::On => 12,
        }
    }
}

#[pymethods]
impl PyPriority {
    #[staticmethod]
    fn schedule() -> Self {
        PyPriority::Schedule
    }

    #[staticmethod]
    fn user_override() -> Self {
        PyPriority::UserOverride
    }

    #[staticmethod]
    fn grid() -> Self {
        PyPriority::Grid
    }

    #[staticmethod]
    fn safety() -> Self {
        PyPriority::Safety
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "schedule" => Ok(PyPriority::Schedule),
            "user_override" | "useroverride" => Ok(PyPriority::UserOverride),
            "grid" => Ok(PyPriority::Grid),
            "safety" => Ok(PyPriority::Safety),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid Priority variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyPriority::Schedule => "Schedule".to_string(),
            PyPriority::UserOverride => "UserOverride".to_string(),
            PyPriority::Grid => "Grid".to_string(),
            PyPriority::Safety => "Safety".to_string(),
        }
    }
}

#[pymethods]
impl PyDRLevel {
    #[staticmethod]
    fn normal() -> Self {
        PyDRLevel::Normal
    }

    #[staticmethod]
    fn moderate() -> Self {
        PyDRLevel::Moderate
    }

    #[staticmethod]
    fn high() -> Self {
        PyDRLevel::High
    }

    #[staticmethod]
    fn critical() -> Self {
        PyDRLevel::Critical
    }

    #[staticmethod]
    fn grid_emergency() -> Self {
        PyDRLevel::GridEmergency
    }

    #[staticmethod]
    fn from_str(name: &str) -> PyResult<Self> {
        match name.to_lowercase().as_str() {
            "normal" => Ok(PyDRLevel::Normal),
            "moderate" => Ok(PyDRLevel::Moderate),
            "high" => Ok(PyDRLevel::High),
            "critical" => Ok(PyDRLevel::Critical),
            "grid_emergency" | "gridemergency" => Ok(PyDRLevel::GridEmergency),
            _ => Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "invalid DRLevel variant: {}",
                name
            ))),
        }
    }

    fn __repr__(&self) -> String {
        format!("{:?}", self)
    }

    fn __str__(&self) -> String {
        match self {
            PyDRLevel::Normal => "Normal".to_string(),
            PyDRLevel::Moderate => "Moderate".to_string(),
            PyDRLevel::High => "High".to_string(),
            PyDRLevel::Critical => "Critical".to_string(),
            PyDRLevel::GridEmergency => "GridEmergency".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_core::Actor;

    #[test]
    fn max_capacity_fraction_signal_converts_to_control_signal() {
        let signal = PySignal::MaxCapacityFraction { fraction: 0.8 };
        let cs = signal.into_control_signal();
        assert!(matches!(
            cs,
            ControlSignal::MaxCapacityFraction { fraction: f } if f == 0.8
        ));
    }

    #[test]
    fn max_capacity_fraction_dispatch_request_constructs() {
        let req = PyDispatchRequest {
            target: "test_equip".to_string(),
            signal: PySignal::MaxCapacityFraction { fraction: 0.6 },
            priority: PyPriority::UserOverride,
        };
        let dr = req.into_dispatch_request();
        assert!(matches!(
            dr.target,
            DispatchTarget::ByName(ref name) if name.as_ref() == "test_equip"
        ));
        assert!(matches!(
            dr.signal,
            ControlSignal::MaxCapacityFraction { fraction: f } if f == 0.6
        ));
    }

    #[test]
    fn duty_cycle_with_component_round_trips_through_into_control_signal() {
        let signal = PySignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: Some(PyDutyCycleComponent::Compressor),
        };
        let cs = signal.into_control_signal();
        assert!(matches!(
            cs,
            ControlSignal::DutyCycle {
                on_fraction,
                component: Some(DutyCycleComponent::Compressor),
                ..
            } if (on_fraction - 0.5).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn duty_cycle_with_backup_element_round_trips() {
        let signal = PySignal::DutyCycle {
            on_fraction: 0.3,
            period_s: None,
            component: Some(PyDutyCycleComponent::BackupElement),
        };
        let cs = signal.into_control_signal();
        assert!(matches!(
            cs,
            ControlSignal::DutyCycle {
                on_fraction,
                component: Some(DutyCycleComponent::BackupElement),
                ..
            } if (on_fraction - 0.3).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn duty_cycle_without_component_round_trips() {
        let signal = PySignal::DutyCycle {
            on_fraction: 0.7,
            period_s: Some(300.0),
            component: None,
        };
        let cs = signal.into_control_signal();
        assert!(matches!(
            cs,
            ControlSignal::DutyCycle {
                on_fraction,
                period_s: Some(300.0),
                component: None,
            } if (on_fraction - 0.7).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn duty_cycle_with_component_round_trips_through_dispatch_request() {
        let req = PyDispatchRequest {
            target: "hpwh".to_string(),
            signal: PySignal::DutyCycle {
                on_fraction: 0.3,
                period_s: None,
                component: Some(PyDutyCycleComponent::BackupElement),
            },
            priority: PyPriority::Schedule,
        };
        let dr = req.into_dispatch_request();
        assert!(matches!(
            dr.target,
            DispatchTarget::ByName(ref name) if name.as_ref() == "hpwh"
        ));
        assert!(matches!(
            dr.signal,
            ControlSignal::DutyCycle {
                on_fraction,
                component: Some(DutyCycleComponent::BackupElement),
                ..
            } if (on_fraction - 0.3).abs() < f64::EPSILON
        ));
    }

    fn ensure_python<F>(f: F)
    where
        F: for<'py> FnOnce(Python<'py>),
    {
        // The `auto-initialize` feature performs guarded, idempotent
        // interpreter startup on first attach (a OnceLock + the import
        // lock). Calling `ffi::Py_Initialize()` directly races that guard:
        // under parallel tests it fails with "global import state already
        // initialized", and the follow-on `Python::assume_attached()`
        // without the GIL segfaulted. `Python::attach` is the only sound
        // entry point here.
        Python::attach(f);
    }

    #[test]
    fn py_actor_wrapper_healthy_returns_true_when_no_error() {
        ensure_python(|py| {
            let actor_obj = Py::new(py, PyActor::new()).expect("could not create PyActor");
            let wrapper = PyActorWrapper::new(py, actor_obj);
            assert!(wrapper.healthy(), "fresh PyActorWrapper should be healthy");
            assert!(
                wrapper.last_error.is_none(),
                "fresh PyActorWrapper should have no error"
            );
        });
    }

    #[test]
    fn py_actor_wrapper_healthy_returns_false_after_decide_raises() {
        ensure_python(|py| {
            let actor_obj = Py::new(py, PyActor::new()).expect("could not create PyActor");
            let mut wrapper = PyActorWrapper::new(py, actor_obj);

            let env = hares_core::actor::testing::test_env().build();
            let mut out = Vec::new();
            // PyActor base class has no decide() method — calling it will
            // raise an AttributeError which triggers the error path.
            wrapper.decide(&env, &mut out);

            assert!(
                !wrapper.healthy(),
                "PyActorWrapper should be unhealthy after decide() raises"
            );
            assert!(
                wrapper.last_error.is_some(),
                "PyActorWrapper should have last_error set after decide() raises"
            );
            let err_msg = wrapper.last_error.as_ref().unwrap();
            assert!(
                err_msg.contains("AttributeError"),
                "last_error should contain AttributeError, got: {err_msg}"
            );
            assert!(
                out.is_empty(),
                "dispatch buffer should be empty after decide() raises"
            );
        });
    }

    #[test]
    fn py_actor_decide_executes_from_rust_driven_thread_without_pre_held_gil() {
        // Create the wrapper on the main thread while holding the GIL.
        // After Python::attach returns the GIL is released, so the
        // spawned thread starts without the GIL held. The decide() call
        // uses Python::attach internally and must acquire it cleanly.
        let wrapper = Python::attach(|py| {
            let actor_obj = Py::new(py, PyActor::new()).expect("could not create PyActor");
            PyActorWrapper::new(py, actor_obj)
        });

        let handle = std::thread::spawn(move || {
            let mut wrapper = wrapper;
            let env = hares_core::actor::testing::test_env().build();
            let mut out = Vec::new();

            // This is the key assertion: decide() must succeed even though
            // this thread started without the GIL. Python::attach in
            // decide() acquires the GIL, runs the Python method, and
            // releases it — all transparently.
            wrapper.decide(&env, &mut out);

            // PyActor base class has no decide() method; calling it raises
            // AttributeError which the wrapper converts to last_error.
            assert!(
                !wrapper.healthy(),
                "PyActorWrapper should be unhealthy after decide() raises"
            );
            assert!(
                wrapper.last_error.is_some(),
                "PyActorWrapper should have last_error set after decide() raises"
            );
            let err_msg = wrapper.last_error.as_ref().unwrap();
            assert!(
                err_msg.contains("AttributeError"),
                "last_error should contain AttributeError, got: {err_msg}"
            );
            assert!(
                out.is_empty(),
                "dispatch buffer should be empty after decide() raises"
            );
        });

        handle.join().expect("spawned thread panicked");
    }
}
