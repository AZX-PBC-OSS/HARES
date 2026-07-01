//! Python bindings for Gymnasium RL environment.

use std::collections::HashMap;

use hares_types::ControlSignal;
use hares_types::panic_hook::{self, PanicHookGuard};
use pyo3::Python;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rayon::prelude::*;

#[cfg(feature = "observe")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::py_control::PyControlSignal;
use crate::py_dwelling::{FATAL_DWELLING_PREFIX, PyDwelling};

/// One batched RL step result.
#[derive(Debug, Clone)]
struct StepResult {
    obs: Vec<f64>,
    reward: f64,
    terminated: bool,
    truncated: bool,
    info: HashMap<String, f64>,
    error_msg: Option<String>,
}

fn observation_for_fields(dwelling: &PyDwelling, fields: &[String]) -> Result<Vec<f64>, String> {
    let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
    let guard = dwelling.acquire_string()?;
    guard
        .telemetry()
        .to_observation_vec(&refs)
        .map_err(|err| err.to_string())
}

/// Thin wrapper around a raw pointer to [`PyDwelling`] that is `Send + Sync`.
///
/// # Safety
///
/// The caller must ensure:
/// 1. The underlying `PyDwelling` outlives all uses of this wrapper (guaranteed
///    by the `Py<PyDwelling>` handles kept alive in the calling scope).
/// 2. All methods called through this pointer are internally synchronised
///    (`step_core` and `observation` both lock `Mutex<Dwelling>`).
struct SendDwellingPtr(*const PyDwelling);
unsafe impl Send for SendDwellingPtr {}
unsafe impl Sync for SendDwellingPtr {}

/// Field-specific valid range for action clipping.
///
/// Maps an action field name (case-insensitive) to its (low, high) bound,
/// matching the ranges used by Python `_field_bounds` in `gym_env.py`.
fn field_bounds(field: &str) -> (f64, f64) {
    match field.to_lowercase().as_str() {
        "soc"
        | "target_soc"
        | "min_soc"
        | "max_soc"
        | "fraction"
        | "load_fraction"
        | "on_fraction"
        | "duty_cycle"
        | "target_rh"
        | "min_rh"
        | "max_rh"
        | "connected"
        | "enabled"
        | "solar_only_charging" => (0.0, 1.0),
        "setpoint_c" | "heat_c" | "cool_c" | "heating_setpoint_c" | "cooling_setpoint_c" => {
            (-50.0, 80.0)
        }
        "deadband_c" => (0.0, 30.0),
        _ => (-1.0e6, 1.0e6),
    }
}

/// Build a [`ControlSignal`] from a signal type name and field values.
///
/// Mirrors Python `_build_control_signal` in `gym_env.py`. All branches
/// construct typed [`ControlSignal`] variants directly.
fn build_control_signal(
    signal_type: &str,
    values: &HashMap<String, f64>,
) -> Result<ControlSignal, String> {
    let lower: HashMap<String, f64> = values.iter().map(|(k, v)| (k.to_lowercase(), *v)).collect();

    let signal = match signal_type {
        "ThermalSetpoint" => {
            let heat_c = lower
                .get("heating_setpoint_c")
                .or_else(|| lower.get("heat_c"))
                .or_else(|| lower.get("setpoint_c"))
                .copied();
            let cool_c = lower
                .get("cooling_setpoint_c")
                .or(lower.get("cool_c"))
                .copied();
            let deadband_c = lower.get("deadband_c").copied();
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: heat_c,
                cooling_setpoint_c: cool_c,
                deadband_c,
            }
        }
        "PowerSetpoint" => {
            let active_power_kw = lower
                .get("active_power_kw")
                .or_else(|| lower.get("p_setpoint_kw"))
                .or_else(|| lower.get("kw"))
                .copied()
                .unwrap_or(0.0);
            let reactive_power_kvar = lower.get("reactive_power_kvar").copied();
            ControlSignal::PowerSetpoint {
                active_power_kw,
                reactive_power_kvar,
                min_soc: None,
                max_soc: None,
            }
        }
        "PowerLimit" => {
            let max_power_kw = lower.get("max_power_kw").copied().unwrap_or(0.0);
            let ramp_rate_kw_per_s = lower.get("ramp_rate_kw_per_s").copied();
            ControlSignal::PowerLimit {
                max_power_kw,
                ramp_rate_kw_per_s,
            }
        }
        "SOCTarget" => {
            let target_soc = lower
                .get("target_soc")
                .or_else(|| lower.get("soc"))
                .copied()
                .unwrap_or(0.0);
            let min_soc = lower.get("min_soc").copied();
            let max_soc = lower.get("max_soc").copied();
            ControlSignal::SOCTarget {
                target_soc,
                min_soc,
                max_soc,
            }
        }
        "LoadFraction" => {
            let fraction = lower
                .get("fraction")
                .or_else(|| lower.get("load_fraction"))
                .copied()
                .unwrap_or(0.0);
            ControlSignal::LoadFraction { fraction }
        }
        "DutyCycle" => {
            let on_fraction = lower
                .get("on_fraction")
                .or_else(|| lower.get("duty_cycle"))
                .copied()
                .unwrap_or(0.0);
            let period_s = lower.get("period_s").copied();
            ControlSignal::DutyCycle {
                on_fraction,
                period_s,
                component: None,
            }
        }
        "HumiditySetpoint" => {
            let target_rh = lower.get("target_rh").copied().unwrap_or(0.0);
            let min_rh = lower.get("min_rh").copied();
            let max_rh = lower.get("max_rh").copied();
            ControlSignal::HumiditySetpoint {
                target_rh,
                min_rh,
                max_rh,
            }
        }
        "GridConnect" => {
            let connected = lower.get("connected").copied().unwrap_or(0.0) >= 0.5;
            ControlSignal::GridConnect { connected }
        }
        "SelfConsumption" => {
            let enabled = lower.get("enabled").copied().unwrap_or(0.0) >= 0.5;
            let solar_only_charging =
                lower.get("solar_only_charging").copied().unwrap_or(0.0) >= 0.5;
            ControlSignal::SelfConsumption {
                enabled,
                solar_only_charging,
            }
        }
        other => {
            return Err(format!("unsupported signal type: {other:?}"));
        }
    };

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        assert_no_nan_inf_in_signal(&signal);
    }

    Ok(signal)
}

/// Cumulative count of action values that exceeded declared bounds and were clipped.
#[cfg(feature = "observe")]
static ACTIONS_CLIPPED: AtomicU64 = AtomicU64::new(0);

/// Throttle gate so the clipping warning is emitted once per process lifetime.
#[cfg(feature = "observe")]
static ACTIONS_CLIPPED_WARNED: AtomicBool = AtomicBool::new(false);

/// Map a flat action vector to per-equipment [`ControlSignal`]s.
///
/// Each action dimension maps to an (equipment, field) pair from `action_layout`.
/// Values are clipped to field-specific bounds, grouped by equipment, and converted
/// to typed signals using `signal_type_by_equipment`.
fn map_action_to_signals(
    action: &[f64],
    action_layout: &[(String, String)],
    signal_type_by_equipment: &HashMap<String, String>,
) -> Result<Vec<(String, ControlSignal)>, String> {
    if action.len() != action_layout.len() {
        return Err(format!(
            "action length ({}) must match action_layout length ({})",
            action.len(),
            action_layout.len(),
        ));
    }

    let mut field_values: HashMap<String, HashMap<String, f64>> = HashMap::new();
    #[cfg(feature = "observe")]
    let mut clipped_count: u64 = 0;
    for (idx, (equipment, field)) in action_layout.iter().enumerate() {
        let raw_value = action[idx];
        if !raw_value.is_finite() {
            return Err(format!(
                "action value at index {idx} for {equipment}.{field} is not finite: {raw_value}",
            ));
        }
        let (low, high) = field_bounds(field);
        let clipped = raw_value.clamp(low, high);
        if clipped != raw_value {
            #[cfg(feature = "observe")]
            {
                clipped_count += 1;
            }
        }

        #[cfg(feature = "observe")]
        {
            let sig_kind = signal_type_by_equipment
                .get(equipment.as_str())
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            tracing::trace!(
                action_index = idx,
                equipment = equipment.as_str(),
                field = field.as_str(),
                raw_value = raw_value,
                clipped_value = clipped,
                signal_variant = sig_kind.as_str(),
                "batch_step action mapping",
            );
        }

        field_values
            .entry(equipment.clone())
            .or_default()
            .insert(field.clone(), clipped);
    }

    #[cfg(feature = "observe")]
    {
        if clipped_count > 0 {
            ACTIONS_CLIPPED.fetch_add(clipped_count, Ordering::Relaxed);
            if ACTIONS_CLIPPED_WARNED
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                tracing::warn!(
                    actions_clipped_this_call = clipped_count,
                    total_actions_clipped = ACTIONS_CLIPPED.load(Ordering::Relaxed),
                    "Action values exceeded declared bounds and were clipped; the RL agent is exploring outside the declared action space. Subsequent clipping events will be suppressed."
                );
            }
        }
    }

    let consumed: usize = field_values.values().map(|m| m.len()).sum();
    if consumed != action.len() {
        return Err(format!(
            "duplicate action layout entries: {consumed} unique fields for {} total dimensions",
            action.len()
        ));
    }

    let mut signals = Vec::new();
    for (equipment, values) in &field_values {
        let signal_type = signal_type_by_equipment
            .get(equipment.as_str())
            .ok_or_else(|| format!("no signal type mapping for equipment {equipment:?}"))?;
        let signal = build_control_signal(signal_type, values)?;
        signals.push((equipment.clone(), signal));
    }

    Ok(signals)
}

/// Assert that no NaN or infinite values are present in a [`ControlSignal`].
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn assert_no_nan_inf_in_signal(signal: &ControlSignal) {
    let check = |name: &str, v: f64| {
        assert!(
            v.is_finite(),
            "{name} must be finite in {signal:?}, got {v}",
        );
    };
    let check_opt = |name: &str, v: Option<f64>| {
        if let Some(value) = v {
            check(name, value);
        }
    };
    match signal {
        ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            deadband_c,
        } => {
            check_opt("heating_setpoint_c", *heating_setpoint_c);
            check_opt("cooling_setpoint_c", *cooling_setpoint_c);
            check_opt("deadband_c", *deadband_c);
        }
        ControlSignal::HumiditySetpoint {
            target_rh,
            min_rh,
            max_rh,
        } => {
            check("target_rh", *target_rh);
            check_opt("min_rh", *min_rh);
            check_opt("max_rh", *max_rh);
        }
        ControlSignal::PowerSetpoint {
            active_power_kw,
            reactive_power_kvar,
            ..
        } => {
            check("active_power_kw", *active_power_kw);
            check_opt("reactive_power_kvar", *reactive_power_kvar);
        }
        ControlSignal::PowerLimit {
            max_power_kw,
            ramp_rate_kw_per_s,
        } => {
            check("max_power_kw", *max_power_kw);
            check_opt("ramp_rate_kw_per_s", *ramp_rate_kw_per_s);
        }
        ControlSignal::SOCTarget {
            target_soc,
            min_soc,
            max_soc,
        } => {
            check("target_soc", *target_soc);
            check_opt("min_soc", *min_soc);
            check_opt("max_soc", *max_soc);
        }
        ControlSignal::DutyCycle { on_fraction, .. } => {
            check("on_fraction", *on_fraction);
        }
        ControlSignal::LoadFraction { fraction } => {
            check("fraction", *fraction);
        }
        _ => {}
    }
}

/// Python-exposed batched step entrypoint.
///
/// Binds all dwelling handles while holding the GIL, maps actions to
/// `ControlSignal`s, applies them to each dwelling's equipment, then
/// releases the GIL so Rayon threads can step dwellings in true parallel.
/// The GIL is only re-acquired afterwards for Python dict construction.
#[pyfunction(name = "batch_step")]
#[pyo3(signature = (dwellings, actions, observation_fields, action_layout, signal_type_by_equipment))]
pub fn batch_step_py(
    py: Python<'_>,
    dwellings: Vec<Py<PyDwelling>>,
    actions: Vec<Vec<f64>>,
    observation_fields: Vec<String>,
    action_layout: Vec<(String, String)>,
    signal_type_by_equipment: HashMap<String, String>,
) -> PyResult<Vec<Py<PyAny>>> {
    // Dimension check first (useful error regardless of mapping status).
    if !actions.is_empty() && actions.len() != dwellings.len() {
        return Err(PyValueError::new_err(format!(
            "actions length ({}) must match dwellings length ({})",
            actions.len(),
            dwellings.len(),
        )));
    }

    // Hold borrows alive for the duration of the parallel section.
    // The Py<PyDwelling> vec keeps every object alive for the entire function.
    let borrows: Vec<PyRef<'_, PyDwelling>> =
        dwellings.iter().map(|d| d.bind(py).borrow()).collect();

    // Map actions to ControlSignals and apply them before entering GIL-free section.
    // Capability validation is enforced by apply_control -> apply_control_validated.
    // NaN/inf guarding is enforced unconditionally in map_action_to_signals
    // and as a defense-in-depth invariant check in build_control_signal.
    for (i, dwelling_ref) in borrows.iter().enumerate() {
        let action = &actions[i];
        if action.is_empty() {
            continue;
        }
        let signals = map_action_to_signals(action, &action_layout, &signal_type_by_equipment)
            .map_err(PyValueError::new_err)?;
        for (equipment, signal) in &signals {
            let py_signal = PyControlSignal {
                signal: signal.clone(),
            };
            dwelling_ref.apply_control(equipment.clone(), &py_signal)?;
        }
    }

    let ptrs: Vec<SendDwellingPtr> = borrows
        .iter()
        .map(|b| SendDwellingPtr(&**b as *const PyDwelling))
        .collect();
    // borrows live until end of function scope

    // Release GIL -- Rayon threads run step_core()/observation() without
    // touching Python. Both methods use only Mutex<Dwelling> internally.
    let _guard = PanicHookGuard::new();
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        assert!(
            panic_hook::is_installed(),
            "custom panic hook must be installed before batch step"
        );
    }
    let results: Vec<StepResult> = py.detach(|| {
        ptrs.par_iter()
            .enumerate()
            .map(|(_idx, SendDwellingPtr(ptr))| {
                // SAFETY: the PyRef borrows in `borrows` and Py<PyDwelling> handles keep the
                // objects alive. step_core() and observation() are GIL-free
                // (they only lock the internal Mutex<Dwelling>).
                let dwelling = unsafe { &**ptr };

                let mut info = HashMap::new();
                let step_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dwelling.step_core_string()
                }));
                let step_result = match step_result {
                    Ok(inner) => inner,
                    Err(payload) => {
                        if dwelling.dwelling.is_poisoned() {
                            dwelling.mark_poisoned();
                        }
                        Err(format!(
                            "HARES internal panic: {}",
                            panic_hook::panic_payload_to_string(payload)
                        ))
                    }
                };
                match step_result {
                    Ok(step) => {
                        let (obs, obs_err) =
                            match observation_for_fields(dwelling, &observation_fields) {
                                Ok(o) => (o, None),
                                Err(e) => {
                                    info.insert("observation_error".to_string(), 1.0);
                                    (dwelling.observation().unwrap_or_default(), Some(e))
                                }
                            };
                        let reward = -step.net_electric_power_kw;
                        info.insert(
                            "net_electric_power_kw".to_string(),
                            step.net_electric_power_kw,
                        );
                        StepResult {
                            obs,
                            reward,
                            terminated: false,
                            truncated: false,
                            info,
                            error_msg: obs_err,
                        }
                    }
                    Err(e) => {
                        let is_fatal = e.starts_with(FATAL_DWELLING_PREFIX);
                        info.insert("error".to_string(), 1.0);
                        if is_fatal {
                            info.insert("dwelling_fatal".to_string(), 1.0);
                        }
                        StepResult {
                            obs: Vec::new(),
                            reward: 0.0,
                            terminated: true,
                            truncated: false,
                            info,
                            error_msg: Some(e),
                        }
                    }
                }
            })
            .collect()
    });

    // GIL re-acquired here -- convert results to Python dicts.
    let mut out = Vec::with_capacity(results.len());
    for item in results {
        let d = PyDict::new(py);
        d.set_item("obs", item.obs)?;
        d.set_item("reward", item.reward)?;
        d.set_item("terminated", item.terminated)?;
        d.set_item("truncated", item.truncated)?;
        let info_dict = PyDict::new(py);
        for (k, v) in &item.info {
            info_dict.set_item(k, v)?;
        }
        if let Some(ref msg) = item.error_msg {
            info_dict.set_item("error_msg", msg)?;
        }
        d.set_item("info", info_dict)?;
        out.push(d.unbind().into());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn therm_layout() -> (Vec<(String, String)>, HashMap<String, String>) {
        let layout = vec![
            ("HVAC".to_string(), "heating_setpoint_c".to_string()),
            ("HVAC".to_string(), "cooling_setpoint_c".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("HVAC".to_string(), "ThermalSetpoint".to_string());
        (layout, sig_map)
    }

    #[test]
    fn field_bounds_soc_range_fraction_fields() {
        assert_eq!(field_bounds("SOC"), (0.0, 1.0));
        assert_eq!(field_bounds("target_soc"), (0.0, 1.0));
        assert_eq!(field_bounds("fraction"), (0.0, 1.0));
        assert_eq!(field_bounds("on_fraction"), (0.0, 1.0));
        assert_eq!(field_bounds("target_rh"), (0.0, 1.0));
    }

    #[test]
    fn field_bounds_binary_fields() {
        assert_eq!(field_bounds("connected"), (0.0, 1.0));
        assert_eq!(field_bounds("enabled"), (0.0, 1.0));
    }

    #[test]
    fn field_bounds_temperature_fields() {
        assert_eq!(field_bounds("heating_setpoint_c"), (-50.0, 80.0));
        assert_eq!(field_bounds("cooling_setpoint_c"), (-50.0, 80.0));
        assert_eq!(field_bounds("heat_c"), (-50.0, 80.0));
    }

    #[test]
    fn field_bounds_deadband() {
        assert_eq!(field_bounds("deadband_c"), (0.0, 30.0));
    }

    #[test]
    fn field_bounds_unknown_falls_back_to_broad() {
        assert_eq!(field_bounds("max_power_kw"), (-1.0e6, 1.0e6));
    }

    #[test]
    fn build_thermal_setpoint_with_all_fields() {
        let mut values = HashMap::new();
        values.insert("heating_setpoint_c".to_string(), 20.0);
        values.insert("cooling_setpoint_c".to_string(), 24.0);
        values.insert("deadband_c".to_string(), 1.0);
        let sig = build_control_signal("ThermalSetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: Some(1.0),
            }
        );
    }

    #[test]
    fn build_thermal_setpoint_with_aliases() {
        let mut values = HashMap::new();
        values.insert("heat_c".to_string(), 18.0);
        let sig = build_control_signal("ThermalSetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(18.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            }
        );
    }

    #[test]
    fn build_power_setpoint() {
        let mut values = HashMap::new();
        values.insert("active_power_kw".to_string(), 3.5);
        let sig = build_control_signal("PowerSetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::PowerSetpoint {
                active_power_kw: 3.5,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            }
        );
    }

    #[test]
    fn build_power_setpoint_defaults_to_zero() {
        let values = HashMap::new();
        let sig = build_control_signal("PowerSetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            }
        );
    }

    #[test]
    fn build_power_limit() {
        let mut values = HashMap::new();
        values.insert("max_power_kw".to_string(), 5.0);
        let sig = build_control_signal("PowerLimit", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::PowerLimit {
                max_power_kw: 5.0,
                ramp_rate_kw_per_s: None,
            }
        );
    }

    #[test]
    fn build_soc_target() {
        let mut values = HashMap::new();
        values.insert("target_soc".to_string(), 0.7);
        values.insert("min_soc".to_string(), 0.2);
        values.insert("max_soc".to_string(), 0.9);
        let sig = build_control_signal("SOCTarget", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::SOCTarget {
                target_soc: 0.7,
                min_soc: Some(0.2),
                max_soc: Some(0.9),
            }
        );
    }

    #[test]
    fn build_soc_target_alias_soc() {
        let mut values = HashMap::new();
        values.insert("soc".to_string(), 0.5);
        let sig = build_control_signal("SOCTarget", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::SOCTarget {
                target_soc: 0.5,
                min_soc: None,
                max_soc: None,
            }
        );
    }

    #[test]
    fn build_load_fraction() {
        let mut values = HashMap::new();
        values.insert("load_fraction".to_string(), 0.8);
        let sig = build_control_signal("LoadFraction", &values).unwrap();
        assert_eq!(sig, ControlSignal::LoadFraction { fraction: 0.8 });
    }

    #[test]
    fn build_duty_cycle() {
        let mut values = HashMap::new();
        values.insert("on_fraction".to_string(), 0.5);
        values.insert("period_s".to_string(), 900.0);
        let sig = build_control_signal("DutyCycle", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::DutyCycle {
                on_fraction: 0.5,
                period_s: Some(900.0),
                component: None,
            }
        );
    }

    #[test]
    fn build_humidity_setpoint() {
        let mut values = HashMap::new();
        values.insert("target_rh".to_string(), 0.45);
        let sig = build_control_signal("HumiditySetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::HumiditySetpoint {
                target_rh: 0.45,
                min_rh: None,
                max_rh: None,
            }
        );
    }

    #[test]
    fn build_grid_connect_threshold() {
        let mut values = HashMap::new();
        values.insert("connected".to_string(), 0.7);
        let sig = build_control_signal("GridConnect", &values).unwrap();
        assert_eq!(sig, ControlSignal::GridConnect { connected: true });

        let mut values = HashMap::new();
        values.insert("connected".to_string(), 0.3);
        let sig = build_control_signal("GridConnect", &values).unwrap();
        assert_eq!(sig, ControlSignal::GridConnect { connected: false });
    }

    #[test]
    fn build_self_consumption() {
        let mut values = HashMap::new();
        values.insert("enabled".to_string(), 0.9);
        values.insert("solar_only_charging".to_string(), 0.1);
        let sig = build_control_signal("SelfConsumption", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::SelfConsumption {
                enabled: true,
                solar_only_charging: false,
            }
        );
    }

    #[test]
    fn build_unknown_signal_type_errors() {
        let values = HashMap::new();
        let err = build_control_signal("UnknownSignal", &values).unwrap_err();
        assert!(
            err.contains("unsupported signal type"),
            "error should mention unsupported signal type, got: {err}"
        );
    }

    #[test]
    fn map_single_equipment_thermal_action() {
        let (layout, sig_map) = therm_layout();
        let action = vec![21.0, 26.0];
        let signals = map_action_to_signals(&action, &layout, &sig_map).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].0, "HVAC");
        assert_eq!(
            signals[0].1,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: Some(26.0),
                deadband_c: None,
            }
        );
    }

    #[test]
    fn map_action_clips_to_bounds() {
        let (layout, sig_map) = therm_layout();
        let action = vec![-100.0, 200.0];
        let signals = map_action_to_signals(&action, &layout, &sig_map).unwrap();
        assert_eq!(
            signals[0].1,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(-50.0),
                cooling_setpoint_c: Some(80.0),
                deadband_c: None,
            }
        );
    }

    #[test]
    fn map_empty_action_is_noop() {
        let (layout, sig_map) = therm_layout();
        let action: Vec<f64> = vec![];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(err.contains("must match action_layout length"));
    }

    #[test]
    fn map_multi_equipment_action() {
        let layout = vec![
            ("HVAC".to_string(), "heat_c".to_string()),
            ("Battery".to_string(), "active_power_kw".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("HVAC".to_string(), "ThermalSetpoint".to_string());
        sig_map.insert("Battery".to_string(), "PowerSetpoint".to_string());
        let action = vec![22.0, -1.5];
        let signals = map_action_to_signals(&action, &layout, &sig_map).unwrap();
        assert_eq!(signals.len(), 2);
        let hvac_sig = signals.iter().find(|(eq, _)| eq == "HVAC").unwrap();
        assert_eq!(
            hvac_sig.1,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(22.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            }
        );
        let batt_sig = signals.iter().find(|(eq, _)| eq == "Battery").unwrap();
        assert_eq!(
            batt_sig.1,
            ControlSignal::PowerSetpoint {
                active_power_kw: -1.5,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            }
        );
    }

    #[test]
    fn map_action_dimension_mismatch_errors() {
        let (layout, sig_map) = therm_layout();
        let action = vec![21.0];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(err.contains("must match action_layout length"));
    }

    #[test]
    fn map_missing_signal_type_errors() {
        let layout = vec![("UnknownEquip".to_string(), "fraction".to_string())];
        let sig_map = HashMap::new();
        let action = vec![0.5];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(err.contains("no signal type mapping"));
    }

    #[test]
    fn build_thermal_setpoint_with_setpoint_c_alias() {
        let mut values = HashMap::new();
        values.insert("setpoint_c".to_string(), 22.0);
        let sig = build_control_signal("ThermalSetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(22.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            }
        );
    }

    #[test]
    fn build_thermal_setpoint_case_insensitive_keys() {
        let mut values = HashMap::new();
        values.insert("Heating_Setpoint_C".to_string(), 20.0);
        values.insert("Cooling_Setpoint_C".to_string(), 24.0);
        let sig = build_control_signal("ThermalSetpoint", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: Some(24.0),
                deadband_c: None,
            }
        );
    }

    #[test]
    fn build_grid_connect_exactly_at_threshold() {
        let mut values = HashMap::new();
        values.insert("connected".to_string(), 0.5);
        let sig = build_control_signal("GridConnect", &values).unwrap();
        assert_eq!(sig, ControlSignal::GridConnect { connected: true });
    }

    #[test]
    fn build_self_consumption_defaults_to_false() {
        let values: HashMap<String, f64> = HashMap::new();
        let sig = build_control_signal("SelfConsumption", &values).unwrap();
        assert_eq!(
            sig,
            ControlSignal::SelfConsumption {
                enabled: false,
                solar_only_charging: false,
            }
        );
    }

    #[test]
    fn map_soctarget_full_layout() {
        let layout = vec![
            ("EV".to_string(), "target_soc".to_string()),
            ("EV".to_string(), "min_soc".to_string()),
            ("EV".to_string(), "max_soc".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("EV".to_string(), "SOCTarget".to_string());
        let action = vec![0.8, 0.1, 0.95];
        let signals = map_action_to_signals(&action, &layout, &sig_map).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(
            signals[0].1,
            ControlSignal::SOCTarget {
                target_soc: 0.8,
                min_soc: Some(0.1),
                max_soc: Some(0.95),
            }
        );
    }

    #[test]
    fn map_dutycycle_full_layout() {
        let layout = vec![
            ("HPWH".to_string(), "on_fraction".to_string()),
            ("HPWH".to_string(), "period_s".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("HPWH".to_string(), "DutyCycle".to_string());
        let action = vec![0.6, 600.0];
        let signals = map_action_to_signals(&action, &layout, &sig_map).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(
            signals[0].1,
            ControlSignal::DutyCycle {
                on_fraction: 0.6,
                period_s: Some(600.0),
                component: None,
            }
        );
    }

    #[test]
    fn map_humidity_setpoint_layout() {
        let layout = vec![("Dehumidifier".to_string(), "target_rh".to_string())];
        let mut sig_map = HashMap::new();
        sig_map.insert("Dehumidifier".to_string(), "HumiditySetpoint".to_string());
        let action = vec![0.5];
        let signals = map_action_to_signals(&action, &layout, &sig_map).unwrap();
        assert_eq!(
            signals[0].1,
            ControlSignal::HumiditySetpoint {
                target_rh: 0.5,
                min_rh: None,
                max_rh: None,
            }
        );
    }

    #[test]
    fn map_action_rejects_nan() {
        let (layout, sig_map) = therm_layout();
        let action = vec![21.0, f64::NAN];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(err.contains("not finite"));
    }

    #[test]
    fn map_action_rejects_infinity() {
        let (layout, sig_map) = therm_layout();
        let action = vec![f64::INFINITY, 26.0];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(err.contains("not finite"));
    }

    #[test]
    fn map_action_rejects_negative_infinity() {
        let (layout, sig_map) = therm_layout();
        let action = vec![21.0, f64::NEG_INFINITY];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(err.contains("not finite"));
    }

    #[test]
    fn map_action_rejects_duplicate_fields_per_equipment() {
        let layout = vec![
            ("HVAC".to_string(), "heat_c".to_string()),
            ("HVAC".to_string(), "heat_c".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("HVAC".to_string(), "ThermalSetpoint".to_string());
        let action = vec![21.0, 22.0];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(
            err.contains("duplicate action layout entries"),
            "error should mention duplicate entries, got: {err}"
        );
    }

    #[test]
    fn map_action_rejects_nan_in_multi_equipment() {
        let layout = vec![
            ("HVAC".to_string(), "heat_c".to_string()),
            ("Battery".to_string(), "active_power_kw".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("HVAC".to_string(), "ThermalSetpoint".to_string());
        sig_map.insert("Battery".to_string(), "PowerSetpoint".to_string());
        let action = vec![22.0, f64::NAN];
        let err = map_action_to_signals(&action, &layout, &sig_map).unwrap_err();
        assert!(
            err.contains("not finite"),
            "error should mention not finite, got: {err}"
        );
    }
}
