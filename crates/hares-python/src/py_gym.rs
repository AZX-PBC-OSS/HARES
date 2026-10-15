//! Python bindings for Gymnasium RL environment.

use std::collections::HashMap;
use std::slice;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
#[cfg(test)]
use std::sync::atomic::Ordering;

use hares_types::ControlSignal;
use hares_types::panic_hook::{self, PanicHookGuard};
use pyo3::Python;
use pyo3::buffer::PyBuffer;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rayon::prelude::*;

use crate::py_control::PyControlSignal;
use crate::py_dwelling::{
    FATAL_DWELLING_PREFIX, PyDwelling, step_result_to_py_dict, zone_keys_for_step,
};

/// The frozen observation plan, built once per episode against a built
/// dwelling and read every step: the Python handle over
/// [`hares_core::ObservationPlan`].
#[pyclass(name = "ObservationPlan")]
pub struct PyObservationPlan {
    pub(crate) plan: Arc<hares_core::ObservationPlan>,
}

#[pymethods]
impl PyObservationPlan {
    /// The number of fields the plan fills, in plan order.
    #[getter]
    pub fn n_fields(&self) -> usize {
        self.plan.len()
    }

    /// The field names the plan was built from, in plan order.
    #[getter]
    pub fn fields(&self) -> Vec<String> {
        self.plan.field_names()
    }

    fn __repr__(&self) -> String {
        format!("ObservationPlan(n_fields={})", self.plan.len())
    }
}

/// Test-only fault switch for the batch observation read: the environment
/// index whose observation read fails. `usize::MAX` faults nobody. Lives
/// in `hares-python` (the crate under test), never in `hares-core`'s
/// `observe_into`: `cfg(test)` is set only for the crate under test.
#[cfg(test)]
pub(crate) static TEST_OBSERVATION_FAULT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// One batched RL step result. The observation is not carried here: the
/// batch writes each environment's row into the caller's buffer, in
/// place, through its buffer protocol.
#[derive(Debug, Clone)]
struct StepResult {
    /// The step dict `PyDwelling.step()` returns, so the caller's
    /// `reward_fn` sees the same context on the batch path as on a single
    /// dwelling's step. `None` when the step failed: a failed step carries
    /// no step dict.
    step: Option<hares_core::StepResult>,
    terminated: bool,
    truncated: bool,
    /// The per-step info entries, `&'static str` keys so a step allocates
    /// no key strings; the dict is built only for the keys set in that
    /// step.
    info: Vec<(&'static str, f64)>,
    error_msg: Option<String>,
    /// Warnings drained from the dwelling after this step (e.g.
    /// control-dispatch rejections). Surfaced under `info["warnings"]` so RL
    /// loops see rejected control signals without polling `take_warnings()`.
    warnings: Vec<String>,
}

/// The batch path reads each environment's observation through this one
/// wrapper around `Dwelling::observe_into`: the plan fills the
/// environment's row of the caller's buffer in place, no per-row lists
/// and no Python object per field.
fn observe_environment_into(
    dwelling: &PyDwelling,
    plan: &hares_core::ObservationPlan,
    out: &mut [f64],
    env_index: usize,
) -> Result<(), String> {
    #[cfg(test)]
    {
        if TEST_OBSERVATION_FAULT.load(Ordering::Relaxed) == env_index {
            return Err(format!(
                "injected observation fault for environment {env_index}"
            ));
        }
    }
    let _ = env_index;
    let guard = dwelling.acquire_string()?;
    guard.observe_into(plan, out).map_err(|err| err.to_string())
}

/// A writable `f64` view of `obj`'s buffer. The batch's observation
/// buffer is the vector environment's own `(n_envs, n_fields)` array,
/// filled in place through its buffer protocol.
fn writable_f64_buffer(obj: &Bound<'_, PyAny>) -> PyResult<PyBuffer<f64>> {
    let buffer = PyBuffer::<f64>::get(obj).map_err(|_| {
        PyValueError::new_err(
            "the observation buffer must expose a writable float64 buffer protocol view",
        )
    })?;
    if buffer.readonly() {
        return Err(PyValueError::new_err(
            "the observation buffer must be writable",
        ));
    }
    Ok(buffer)
}

/// The C-contiguity the flat row writes assume.
fn assert_contiguous_f64(buffer: &PyBuffer<f64>) -> PyResult<()> {
    if !buffer.is_c_contiguous() {
        return Err(PyValueError::new_err(
            "the observation buffer must be C-contiguous",
        ));
    }
    Ok(())
}

/// The observation buffer's base pointer, `Send` for the parallel row
/// writes: the PyBuffer export is held alive across the parallel section,
/// the buffer is writable and C-contiguous, and no other writer touches it
/// while the batch runs.
struct SendObsPtr(*mut f64);
unsafe impl Send for SendObsPtr {}
unsafe impl Sync for SendObsPtr {}

impl Clone for SendObsPtr {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for SendObsPtr {}

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
            hares_types::validate_thermal_setpoint_deadband(heat_c, cool_c, deadband_c)
                .map_err(|err| err.to_string())?;
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

    // A NaN or infinite action value survives clipping: reject it at the
    // boundary with a value error, in every build profile.
    assert_no_nan_inf_in_signal(&signal);

    Ok(signal)
}

/// The engine's declared action-field bounds: one `(low, high)` pair per
/// named field. This is the canonical table: the gym's action space is
/// built from it, every clip path clamps to it, and the gym's Python-side
/// table is pinned to it in both directions.
fn field_bounds_entries() -> &'static [(&'static str, (f64, f64))] {
    &[
        ("connected", (0.0, 1.0)),
        ("cool_c", (-50.0, 80.0)),
        ("cooling_setpoint_c", (-50.0, 80.0)),
        ("duty_cycle", (0.0, 1.0)),
        ("enabled", (0.0, 1.0)),
        ("fraction", (0.0, 1.0)),
        ("heat_c", (-50.0, 80.0)),
        ("heating_setpoint_c", (-50.0, 80.0)),
        ("load_fraction", (0.0, 1.0)),
        ("max_rh", (0.0, 1.0)),
        ("max_soc", (0.0, 1.0)),
        ("min_rh", (0.0, 1.0)),
        ("min_soc", (0.0, 1.0)),
        ("on_fraction", (0.0, 1.0)),
        ("setpoint_c", (-50.0, 80.0)),
        ("soc", (0.0, 1.0)),
        ("solar_only_charging", (0.0, 1.0)),
        ("target_rh", (0.0, 1.0)),
        ("target_soc", (0.0, 1.0)),
    ]
}

/// The declared bounds of one action field, the table the gym's action
/// space is built from and every clip path clamps to.
fn field_bounds(field: &str) -> (f64, f64) {
    let name = field.to_lowercase();
    field_bounds_entries()
        .iter()
        .find(|(known, _)| *known == name)
        .map_or((-1.0e6, 1.0e6), |(_, bounds)| *bounds)
}

/// Clamp each action value to its column's bounds pair and count the clamped
/// values. The counting is unconditional: a run's health carries the clamp
/// count in every build profile.
fn clamp_action(action: &[f64], action_bounds: &[(f64, f64)]) -> (Vec<f64>, u64) {
    let mut clamped_count: u64 = 0;
    let values = action
        .iter()
        .zip(action_bounds)
        .map(|(&raw, &(low, high))| {
            let clipped = raw.clamp(low, high);
            if clipped != raw {
                clamped_count += 1;
            }
            clipped
        })
        .collect();
    (values, clamped_count)
}

/// The bounds of each layout column: a `deadband_c` column is held to the
/// band range of its target's thermostat class in `dwelling`, every other
/// column to the field's declared table bounds.
fn resolve_action_bounds(
    dwelling: &PyDwelling,
    action_layout: &[(String, String)],
) -> Result<Vec<(f64, f64)>, String> {
    action_layout
        .iter()
        .map(|(equipment, field)| {
            if field.eq_ignore_ascii_case("deadband_c") {
                dwelling.band_range_of(equipment)?.ok_or_else(|| {
                    format!(
                        "equipment {equipment:?} has no thermostat band for a deadband_c action"
                    )
                })
            } else {
                Ok(field_bounds(field))
            }
        })
        .collect()
}

/// One finite `(low, high)` pair with `low <= high` per layout column, so
/// clipping to it is defined.
fn validate_action_bounds(
    action_layout: &[(String, String)],
    action_bounds: &[(f64, f64)],
) -> Result<(), String> {
    if action_bounds.len() != action_layout.len() {
        return Err(format!(
            "action_bounds length ({}) must match action_layout length ({})",
            action_bounds.len(),
            action_layout.len(),
        ));
    }
    for ((equipment, field), &(low, high)) in action_layout.iter().zip(action_bounds) {
        let fault = if !low.is_finite() || !high.is_finite() {
            "not finite"
        } else if low > high {
            "low above high"
        } else {
            continue;
        };
        return Err(format!(
            "action_bounds for {equipment}.{field} are ({low}, {high}): {fault}"
        ));
    }
    Ok(())
}

/// The bounds of each `deadband_c` column must lie within the band range
/// of the target's thermostat class in `dwelling`: one bounds list serves
/// every dwelling of a call, so a dwelling whose unit holds a narrower band
/// is refused rather than sent a band it rejects.
fn check_deadband_bounds(
    dwelling: &PyDwelling,
    action_layout: &[(String, String)],
    action_bounds: &[(f64, f64)],
) -> Result<(), String> {
    for ((equipment, field), &(low, high)) in action_layout.iter().zip(action_bounds) {
        if !field.eq_ignore_ascii_case("deadband_c") {
            continue;
        }
        let (min, max) = dwelling.band_range_of(equipment)?.ok_or_else(|| {
            format!("equipment {equipment:?} has no thermostat band for a deadband_c action")
        })?;
        if low < min || high > max {
            return Err(format!(
                "action_bounds for {equipment}.{field} are ({low}, {high}), outside the \
                 ({min}, {max}) band range of its thermostat in this dwelling; bounds are \
                 per call, so every dwelling's unit must hold them"
            ));
        }
    }
    Ok(())
}

/// Refuses, changing nothing, a signal the dwelling's equipment would
/// refuse when it is applied: the equipment is missing, lacks the
/// capability, or its state rejects it. No action maps to an immediate
/// state update, so the apply that follows only queues what passed here.
fn check_signals_accepted(
    dwelling: &PyDwelling,
    signals: &[(String, ControlSignal)],
) -> Result<(), String> {
    let dwelling = dwelling.acquire_string()?;
    for (equipment, signal) in signals {
        debug_assert!(!signal.is_immediate_state_update());
        dwelling
            .check_control(equipment, signal)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Map a flat action vector to per-equipment [`ControlSignal`]s.
///
/// Each action dimension maps to an (equipment, field) pair from `action_layout`.
/// Values are clipped to `action_bounds` (one `(low, high)` per layout column,
/// resolved once by the environment from the field and, for a deadband, the
/// target's thermostat class), grouped by equipment, and converted to typed
/// signals using `signal_type_by_equipment`. Returns the signals with the
/// number of clamped values, which the caller records on the acting
/// dwelling's run health.
fn map_action_to_signals(
    action: &[f64],
    action_layout: &[(String, String)],
    signal_type_by_equipment: &HashMap<String, String>,
    action_bounds: &[(f64, f64)],
) -> Result<(Vec<(String, ControlSignal)>, u64), String> {
    if action.len() != action_layout.len() {
        return Err(format!(
            "action length ({}) must match action_layout length ({})",
            action.len(),
            action_layout.len(),
        ));
    }
    validate_action_bounds(action_layout, action_bounds)?;
    check_actions_finite(action_layout, action)?;

    let (clipped_values, clamped_count) = clamp_action(action, action_bounds);

    #[cfg(feature = "observe")]
    for (idx, (equipment, field)) in action_layout.iter().enumerate() {
        let sig_kind = signal_type_by_equipment
            .get(equipment.as_str())
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        tracing::trace!(
            action_index = idx,
            equipment = equipment.as_str(),
            field = field.as_str(),
            raw_value = action[idx],
            clipped_value = clipped_values[idx],
            signal_variant = sig_kind.as_str(),
            "batch_step action mapping",
        );
    }

    let mut field_values: HashMap<String, HashMap<String, f64>> = HashMap::new();
    for (idx, (equipment, field)) in action_layout.iter().enumerate() {
        field_values
            .entry(equipment.clone())
            .or_default()
            .insert(field.clone(), clipped_values[idx]);
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

    Ok((signals, clamped_count))
}

/// Every action value is finite: a NaN or infinite value survives
/// clipping, so the call is refused with a typed error naming the layout
/// column and the value, in every build profile. A refused call clamps
/// nothing and counts nothing.
fn check_actions_finite(action_layout: &[(String, String)], action: &[f64]) -> Result<(), String> {
    for (idx, (equipment, field)) in action_layout.iter().enumerate() {
        let raw_value = action[idx];
        if !raw_value.is_finite() {
            return Err(format!(
                "action value at index {idx} for {equipment}.{field} is not finite: {raw_value}",
            ));
        }
    }
    Ok(())
}

/// Refuse, changing nothing, a signal holding a NaN or infinite value: a
/// NaN action survives clipping, so the finiteness screen is a typed error
/// in every build.
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
///
/// Each environment's observation is read through the environment's own
/// frozen plan (`plans`, built at the environment's `reset`) into `obs_out`,
/// the vector environment's `(n_envs, n_fields)` float64 buffer, filled in
/// place through its buffer protocol: no per-row lists and no Python object
/// per field. The buffer's row width is the plan's field count.
///
/// Each result dict carries `"step"`, the same dict `Dwelling.step()`
/// returns (both go through one conversion), so the caller's `reward_fn`
/// sees the same context on the batch path as on a single dwelling's step.
/// A failed step carries `"step": None` and keeps the batch rectangular:
/// its row of the buffer holds the dwelling's last observation (the failed
/// step changed nothing, so the telemetry on hand is that state). A step
/// whose observation read fails ends that environment with its error:
/// `terminated=True`, `info["error_msg"]` holding the error, and its row of
/// the buffer set to NaN; no substitute vector is produced. Its `info`
/// dict always carries `"warning_count"` (float), the
/// number of warnings the dwelling accumulated during the step (e.g. control
/// signals rejected at dispatch time). When non-zero, `"warnings"`
/// (list of str) holds the drained messages; they are consumed here, so a
/// subsequent `Dwelling.take_warnings()` will not return them again.
///
/// `action_bounds` holds one finite `(low, high)` with `low <= high` per
/// `action_layout` column, which the environment resolves once from the
/// action space (a deadband by its target's thermostat class); every action
/// is clipped to it. The bounds are per call, not per dwelling: a dwelling
/// whose deadband target holds a narrower band than its column's bounds is
/// refused. Any refusal (a bound, a mapping, or equipment that is missing
/// or would reject its signal) is a `ValueError` raised before a signal
/// reaches any dwelling.
#[pyfunction(name = "batch_step")]
#[pyo3(signature = (dwellings, actions, plans, action_layout, signal_type_by_equipment, action_bounds, obs_out))]
#[allow(clippy::too_many_arguments)]
pub fn batch_step_py(
    py: Python<'_>,
    dwellings: Vec<Py<PyDwelling>>,
    actions: Vec<Vec<f64>>,
    plans: Vec<Py<PyObservationPlan>>,
    action_layout: Vec<(String, String)>,
    signal_type_by_equipment: HashMap<String, String>,
    action_bounds: Vec<(f64, f64)>,
    obs_out: &Bound<'_, PyAny>,
) -> PyResult<Vec<Py<PyAny>>> {
    // Dimension check first (useful error regardless of mapping status).
    if !actions.is_empty() && actions.len() != dwellings.len() {
        return Err(PyValueError::new_err(format!(
            "actions length ({}) must match dwellings length ({})",
            actions.len(),
            dwellings.len(),
        )));
    }
    if plans.len() != dwellings.len() {
        return Err(PyValueError::new_err(format!(
            "plans length ({}) must match dwellings length ({})",
            plans.len(),
            dwellings.len(),
        )));
    }
    // One plan per environment, all freezing the same field list: the
    // observation buffer's width is the plan's, one row per environment.
    let plan_handles: Vec<Arc<hares_core::ObservationPlan>> = plans
        .iter()
        .map(|plan| Arc::clone(&plan.bind(py).borrow().plan))
        .collect();
    let n_fields = plan_handles.first().map(|plan| plan.len()).unwrap_or(0);
    if plan_handles.iter().any(|plan| plan.len() != n_fields) {
        return Err(PyValueError::new_err(
            "the plans must freeze the same field list: one observation buffer serves the batch",
        ));
    }

    // The buffer is the vector environment's own: filled in place, row by
    // row, through its buffer protocol. The PyBuffer keeps the export
    // alive while the parallel section writes through the pointer.
    let obs_buffer = writable_f64_buffer(obs_out)?;
    if obs_buffer.item_count() != dwellings.len() * n_fields {
        return Err(PyValueError::new_err(format!(
            "the observation buffer holds {} floats for {} environments of {} fields",
            obs_buffer.item_count(),
            dwellings.len(),
            n_fields,
        )));
    }
    assert_contiguous_f64(&obs_buffer)?;
    let obs_ptr = SendObsPtr(obs_buffer.buf_ptr().cast::<f64>());
    let obs_row = |env_index: usize, ptr: SendObsPtr, width: usize| -> &mut [f64] {
        // SAFETY: the buffer is writable, C-contiguous, holds
        // dwellings.len() * n_fields floats, and the PyBuffer keeps the
        // export alive for the write's duration; no other writer touches
        // the buffer while the GIL is held or released by this call.
        unsafe { slice::from_raw_parts_mut((ptr.0).add(env_index * width), width) }
    };

    // Hold borrows alive for the duration of the parallel section.
    // The Py<PyDwelling> vec keeps every object alive for the entire function.
    let borrows: Vec<PyRef<'_, PyDwelling>> =
        dwellings.iter().map(|d| d.bind(py).borrow()).collect();

    // Map every dwelling's actions to ControlSignals before applying any, so
    // a refused action leaves every dwelling untouched; then apply them
    // before entering the GIL-free section.
    // Equipment presence, capability and state are checked by
    // check_signals_accepted with the rest, so the apply below cannot refuse.
    // NaN/inf guarding is enforced unconditionally in map_action_to_signals
    // and as a defense-in-depth invariant check in build_control_signal.
    let mut mapped = Vec::with_capacity(borrows.len());
    for (dwelling_ref, action) in borrows.iter().zip(&actions) {
        if action.is_empty() {
            continue;
        }
        check_deadband_bounds(dwelling_ref, &action_layout, &action_bounds)
            .map_err(PyValueError::new_err)?;
        let (signals, clamped_count) = map_action_to_signals(
            action,
            &action_layout,
            &signal_type_by_equipment,
            &action_bounds,
        )
        .map_err(PyValueError::new_err)?;
        check_signals_accepted(dwelling_ref, &signals).map_err(PyValueError::new_err)?;
        mapped.push((dwelling_ref, signals, clamped_count));
    }
    for (dwelling_ref, signals, clamped_count) in mapped {
        // The clamp count is the acting dwelling's own, recorded on its run
        // health in every build, immediately before its signals apply; a
        // refused call above reaches no dwelling and records nothing.
        dwelling_ref
            .record_clamped_actions(clamped_count)
            .map_err(PyValueError::new_err)?;
        for (equipment, signal) in &signals {
            let py_signal = PyControlSignal {
                signal: signal.clone(),
            };
            dwelling_ref.apply_control(equipment.clone(), &py_signal, None)?;
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
    #[cfg(debug_assertions)]
    {
        assert!(
            panic_hook::is_installed(),
            "custom panic hook must be installed before batch step"
        );
    }
    let results: Vec<StepResult> = py.detach(|| {
        ptrs.par_iter()
            .enumerate()
            .map(|(idx, SendDwellingPtr(ptr))| {
                // SAFETY: the PyRef borrows in `borrows` and Py<PyDwelling> handles keep the
                // objects alive. step_core() and observation() are GIL-free
                // (they only lock the internal Mutex<Dwelling>).
                let dwelling = unsafe { &**ptr };

                let mut info: Vec<(&'static str, f64)> = Vec::new();
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
                // Drain warnings accumulated during this step (e.g. control
                // signals rejected at dispatch time) so RL loops see them in
                // `info` without a separate `take_warnings()` poll. A fatally
                // poisoned dwelling yields no warnings; the error path below
                // already reports the fatal state.
                let warnings = dwelling
                    .acquire_string()
                    .map(|mut guard| {
                        guard
                            .take_warnings()
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default();
                info.push(("warning_count", warnings.len() as f64));
                match step_result {
                    Ok(step) => {
                        let plan = &plan_handles[idx];
                        let row = obs_row(idx, obs_ptr, n_fields);
                        if let Err(e) = observe_environment_into(dwelling, plan, row, idx) {
                            // A step whose observation fails ends that
                            // environment with its error: no substitute
                            // vector, the row NaN, the environment done.
                            info.push(("observation_error", 1.0));
                            row.fill(f64::NAN);
                            StepResult {
                                step: Some(step),
                                terminated: true,
                                truncated: false,
                                info,
                                error_msg: Some(e),
                                warnings,
                            }
                        } else {
                            StepResult {
                                step: Some(step),
                                terminated: false,
                                truncated: false,
                                info,
                                error_msg: None,
                                warnings,
                            }
                        }
                    }
                    Err(e) => {
                        let is_fatal = e.starts_with(FATAL_DWELLING_PREFIX);
                        info.push(("error", 1.0));
                        if is_fatal {
                            info.push(("dwelling_fatal", 1.0));
                        }
                        // A failed row carries the dwelling's last
                        // observation: the failed step changed nothing, so
                        // the telemetry on hand is the last known state, and
                        // the batch stays rectangular (one obs row per
                        // dwelling, the same width as the living rows). A
                        // dwelling whose telemetry cannot answer the fields
                        // gets a NaN row of the right width; the step error
                        // stays the headline in `error_msg` and the
                        // observation failure is flagged in `info`.
                        let row = obs_row(idx, obs_ptr, n_fields);
                        if observe_environment_into(dwelling, &plan_handles[idx], row, idx).is_err()
                        {
                            info.push(("observation_error", 1.0));
                            row.fill(f64::NAN);
                        }
                        StepResult {
                            step: None,
                            terminated: true,
                            truncated: false,
                            info,
                            error_msg: Some(e),
                            warnings,
                        }
                    }
                }
            })
            .collect()
    });
    drop(obs_buffer);

    // GIL re-acquired here -- convert results to Python dicts.
    let mut out = Vec::with_capacity(results.len());
    for item in results {
        let d = PyDict::new(py);
        match item.step {
            Some(step) => {
                let zone_keys = zone_keys_for_step(&step);
                d.set_item("step", step_result_to_py_dict(py, &step, &zone_keys)?)?;
            }
            None => d.set_item("step", py.None())?,
        }
        d.set_item("terminated", item.terminated)?;
        d.set_item("truncated", item.truncated)?;
        let info_dict = PyDict::new(py);
        for (k, v) in &item.info {
            info_dict.set_item(k, v)?;
        }
        if let Some(ref msg) = item.error_msg {
            info_dict.set_item("error_msg", msg)?;
        }
        if !item.warnings.is_empty() {
            info_dict.set_item("warnings", &item.warnings)?;
        }
        d.set_item("info", info_dict)?;
        out.push(d.unbind().into());
    }

    Ok(out)
}

/// Clamp an action vector for one dwelling and record the clamped count on
/// its run health, in every build. The single-dwelling gym path clips
/// through here so its counts land in the same place the batch path's do,
/// and builds its signals from the returned values.
#[pyfunction(name = "clip_action")]
#[pyo3(signature = (dwelling, action, action_layout))]
pub fn clip_action_py(
    dwelling: &PyDwelling,
    action: Vec<f64>,
    action_layout: Vec<(String, String)>,
) -> PyResult<Vec<f64>> {
    if action.len() != action_layout.len() {
        return Err(PyValueError::new_err(format!(
            "action length ({}) must match action_layout length ({})",
            action.len(),
            action_layout.len(),
        )));
    }
    let action_bounds =
        resolve_action_bounds(dwelling, &action_layout).map_err(PyValueError::new_err)?;
    // The same screen as the batch path: a non-finite action is a refused
    // call, not a clamp count.
    check_actions_finite(&action_layout, &action).map_err(PyValueError::new_err)?;
    let (values, clamped_count) = clamp_action(&action, &action_bounds);
    dwelling.record_clamped_actions(clamped_count)?;
    Ok(values)
}

/// The declared bounds of one action field, so the gym's Python-side table
/// can be pinned against this one.
#[pyfunction(name = "field_bounds")]
#[pyo3(signature = (field))]
pub fn field_bounds_py(field: &str) -> (f64, f64) {
    field_bounds(field)
}

/// The engine's whole declared action-field bounds table, so the gym's
/// Python-side table can be pinned against this one in both directions:
/// growth on either side fails the pin.
#[pyfunction(name = "field_bounds_table")]
pub fn field_bounds_table_py() -> HashMap<String, (f64, f64)> {
    field_bounds_entries()
        .iter()
        .map(|(field, bounds)| (field.to_string(), *bounds))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use numpy::PyArrayMethods;
    use pyo3::types::PyString;

    /// A stepable base.xml dwelling for the batch health tests.
    fn base_dwelling(py: Python<'_>) -> Py<PyDwelling> {
        use std::path::PathBuf;

        let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        root.push("../..");
        let sim_config = crate::py_config::PySimulationConfig::new(
            "2019-01-01T00:00:00",
            crate::py_config::WholeSeconds(600),
            crate::py_config::WholeSeconds(60),
            None,
            false,
            false,
            hares_io::DEFAULT_OUTPUT_CHUNK_SIZE,
            None,
            0,
            None,
            None,
            hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
            None,
            crate::py_config::PyWarmupArg::Absent,
            "hour24",
            "typical",
            None,
        )
        .expect("the sim config builds");
        let mut dwelling = PyDwelling::from_hpxml(
            &py.get_type::<PyDwelling>(),
            PyString::new(
                py,
                root.join("tests/fixtures/hpxml/ochre_samples/base.xml")
                    .to_str()
                    .expect("utf8"),
            )
            .as_any(),
            Some(
                PyString::new(
                    py,
                    root.join("data/examples/BEopt_example_schedule.csv")
                        .to_str()
                        .expect("utf8"),
                )
                .as_any(),
            ),
            PyString::new(
                py,
                root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
                    .to_str()
                    .expect("utf8"),
            )
            .as_any(),
            sim_config,
            0,
            Some(PyString::new(py, root.join("defaults").to_str().expect("utf8")).as_any()),
            None,
            None,
        )
        .expect("base.xml must load");
        dwelling.initialize().expect("initialize must succeed");
        Py::new(py, dwelling).expect("the dwelling wraps")
    }

    /// Each dwelling's run health records its own action's clamps: the first
    /// row's 100.0 is above the 80.0 bound and counts, the second row's 20.0
    /// is inside and does not. In every build.
    #[test]
    fn batch_step_records_clamped_actions() {
        Python::attach(|py| {
            let first = base_dwelling(py);
            let second = base_dwelling(py);
            let layout = vec![("Gas Furnace".to_string(), "heating_setpoint_c".to_string())];
            let mut signal_types = HashMap::new();
            signal_types.insert("Gas Furnace".to_string(), "ThermalSetpoint".to_string());

            let plans: Vec<Py<PyObservationPlan>> = [&first, &second]
                .into_iter()
                .map(|dwelling| {
                    let plan = dwelling
                        .bind(py)
                        .borrow()
                        .observation_plan(vec!["total_power_kw".to_string()])
                        .expect("the plan builds");
                    Py::new(py, plan).expect("the plan wraps")
                })
                .collect();
            let buffer = numpy::PyArray2::<f64>::zeros(py, (2, 1), false);

            batch_step_py(
                py,
                vec![first.clone_ref(py), second.clone_ref(py)],
                vec![vec![100.0], vec![20.0]],
                plans,
                layout,
                signal_types,
                vec![(-50.0, 80.0)],
                buffer.as_any(),
            )
            .expect("the batch step must run");

            let first_clamped = first
                .bind(py)
                .borrow()
                .acquire_string()
                .expect("not poisoned")
                .health()
                .clamped_actions;
            let second_clamped = second
                .bind(py)
                .borrow()
                .acquire_string()
                .expect("not poisoned")
                .health()
                .clamped_actions;
            assert_eq!(first_clamped, 1, "the 100.0 action is one clamped value");
            assert_eq!(second_clamped, 0, "the in-bounds action clamps nothing");
        });
    }

    /// A step whose observation read fails ends that environment with its
    /// error: `terminated=True`, `info["error_msg"]` holding the error and
    /// its row of the buffer NaN, while the other environment steps
    /// normally. The fault is the test-only switch in the batch's one
    /// observation wrapper, naming the environment index to fail; no
    /// substitute vector is produced.
    #[test]
    fn observation_failure_terminates_the_environment() {
        Python::attach(|py| {
            let first = base_dwelling(py);
            let second = base_dwelling(py);
            let fields = vec!["total_power_kw".to_string()];
            let plans: Vec<Py<PyObservationPlan>> =
                vec![first.bind(py).borrow(), second.bind(py).borrow()]
                    .into_iter()
                    .map(|dwelling| {
                        let plan = dwelling
                            .observation_plan(fields.clone())
                            .expect("the plan builds");
                        Py::new(py, plan).expect("the plan wraps")
                    })
                    .collect();

            let layout = vec![("Gas Furnace".to_string(), "heating_setpoint_c".to_string())];
            let mut signal_types = HashMap::new();
            signal_types.insert("Gas Furnace".to_string(), "ThermalSetpoint".to_string());

            let buffer = numpy::PyArray2::<f64>::zeros(py, (2, 1), false);

            TEST_OBSERVATION_FAULT.store(0, Ordering::Relaxed);
            let result = batch_step_py(
                py,
                vec![first.clone_ref(py), second.clone_ref(py)],
                vec![vec![20.0], vec![20.0]],
                plans,
                layout,
                signal_types,
                vec![(-50.0, 80.0)],
                buffer.as_any(),
            );
            TEST_OBSERVATION_FAULT.store(usize::MAX, Ordering::Relaxed);
            let rows = result.expect("the batch step itself runs");

            let terminated: Vec<bool> = rows
                .iter()
                .map(|row| {
                    row.bind(py)
                        .get_item("terminated")
                        .and_then(|v| v.extract())
                        .expect("terminated is a bool")
                })
                .collect();
            assert!(terminated[0], "the failed observation ends environment 0");
            assert!(!terminated[1], "environment 1 steps normally");

            let error_msg: String = rows[0]
                .bind(py)
                .get_item("info")
                .and_then(|info| info.get_item("error_msg"))
                .and_then(|v| v.extract())
                .expect("the failed environment's info carries its error");
            assert!(
                error_msg.contains("injected observation fault for environment 0"),
                "the error names the environment: {error_msg}"
            );

            let read = buffer.readonly();
            let view = read.as_array();
            assert!(
                view[[0, 0]].is_nan(),
                "the failed environment's row is NaN, got {}",
                view[[0, 0]]
            );
            assert!(
                view[[1, 0]].is_finite(),
                "the stepping environment's row is a real observation"
            );
        });
    }

    /// The single-dwelling clip path returns the clamped values and records
    /// the count on the dwelling's run health, in every build.
    #[test]
    fn clip_action_records_and_returns_clamped_values() {
        Python::attach(|py| {
            let dwelling = base_dwelling(py);
            let bound_dwelling = base_dwelling(py);
            let layout = vec![("Gas Furnace".to_string(), "heating_setpoint_c".to_string())];

            let bound_ref = bound_dwelling.bind(py).borrow();
            let values =
                clip_action_py(&bound_ref, vec![100.0], layout.clone()).expect("the clip must run");
            assert_eq!(values, vec![80.0]);
            let clamped = bound_dwelling
                .bind(py)
                .borrow()
                .acquire_string()
                .expect("not poisoned")
                .health()
                .clamped_actions;
            assert_eq!(clamped, 1, "the clip records one clamped value");

            let dwelling_ref = dwelling.bind(py).borrow();
            let untouched =
                clip_action_py(&dwelling_ref, vec![20.0], layout).expect("the clip must run");
            assert_eq!(untouched, vec![20.0]);
            let untouched_clamped = dwelling
                .bind(py)
                .borrow()
                .acquire_string()
                .expect("not poisoned")
                .health()
                .clamped_actions;
            assert_eq!(untouched_clamped, 0, "an in-bounds action clamps nothing");
        });
    }

    /// A non-finite action on the single-dwelling clip path is a refused
    /// call, the batch path's semantics: the error is typed, and the
    /// dwelling's run health records nothing.
    #[test]
    fn clip_action_refuses_a_non_finite_action() {
        Python::attach(|py| {
            for raw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let dwelling = base_dwelling(py);
                let layout = vec![("Gas Furnace".to_string(), "heating_setpoint_c".to_string())];
                let err = clip_action_py(&dwelling.bind(py).borrow(), vec![raw], layout)
                    .expect_err("a non-finite action is refused");
                assert!(err.is_instance_of::<PyValueError>(py));
                let message = err.value(py).to_string();
                assert!(message.contains("not finite"), "{raw}: {message}");

                let clamped = dwelling
                    .bind(py)
                    .borrow()
                    .acquire_string()
                    .expect("not poisoned")
                    .health()
                    .clamped_actions;
                assert_eq!(clamped, 0, "a refused call records nothing");
            }
        });
    }

    /// The exposed bounds table and the lookup agree: every named field
    /// returns its own entry's bounds, case-insensitively, and a field
    /// neither names falls through to the broad bounds.
    #[test]
    fn field_bounds_table_and_lookup_agree() {
        assert!(!field_bounds_entries().is_empty(), "the table is not empty");
        for (field, bounds) in field_bounds_entries() {
            assert_eq!(field_bounds(field), *bounds, "{field}");
            assert_eq!(field_bounds(&field.to_uppercase()), *bounds, "{field}");
        }
        assert_eq!(field_bounds("mystery_field"), (-1.0e6, 1.0e6));
    }

    fn therm_layout() -> (Vec<(String, String)>, HashMap<String, String>) {
        let layout = vec![
            ("HVAC".to_string(), "heating_setpoint_c".to_string()),
            ("HVAC".to_string(), "cooling_setpoint_c".to_string()),
        ];
        let mut sig_map = HashMap::new();
        sig_map.insert("HVAC".to_string(), "ThermalSetpoint".to_string());
        (layout, sig_map)
    }

    /// The bounds an environment resolves for `layout`: "HVAC" is a cycling
    /// unit and "WH" a storage water heater.
    fn bounds(layout: &[(String, String)]) -> Vec<(f64, f64)> {
        use hares_types::ThermostatBandClass;
        layout
            .iter()
            .map(
                |(equipment, field)| match (equipment.as_str(), field.as_str()) {
                    ("WH", "deadband_c") => ThermostatBandClass::Tank.range_c(),
                    (_, "deadband_c") => ThermostatBandClass::Cycling.range_c(),
                    (_, "heat_c" | "cool_c" | "setpoint_c")
                    | (_, "heating_setpoint_c" | "cooling_setpoint_c") => (-50.0, 80.0),
                    (_, "fraction" | "on_fraction" | "target_soc" | "soc") => (0.0, 1.0),
                    _ => (-1.0e6, 1.0e6),
                },
            )
            .collect()
    }

    /// Each column is clipped to its own resolved bounds, so a deadband is
    /// held to its target's thermostat class.
    #[test]
    fn each_action_is_clipped_to_its_resolved_bounds() {
        let layout = vec![
            ("HVAC".to_string(), "deadband_c".to_string()),
            ("HVAC".to_string(), "heat_c".to_string()),
            ("WH".to_string(), "deadband_c".to_string()),
            ("WH".to_string(), "setpoint_c".to_string()),
        ];
        let sig_map: HashMap<String, String> = ["HVAC", "WH"]
            .into_iter()
            .map(|eq| (eq.to_string(), "ThermalSetpoint".to_string()))
            .collect();
        let action = vec![30.0, 21.0, 30.0, 50.0];
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
        let band_of = |name: &str| match signals.iter().find(|(eq, _)| eq == name) {
            Some((_, ControlSignal::ThermalSetpoint { deadband_c, .. })) => *deadband_c,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            band_of("HVAC"),
            Some(hares_types::MAX_HVAC_THERMOSTAT_BAND_C)
        );
        assert_eq!(band_of("WH"), Some(30.0));

        let err =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout[..1])).unwrap_err();
        assert!(err.contains("action_bounds length"), "{err}");
    }

    /// A bounds pair that is not finite or is inverted is a typed error,
    /// never a clamp panic.
    #[test]
    fn a_bad_bounds_pair_is_an_error_not_a_panic() {
        let (layout, sig_map) = therm_layout();
        let action = vec![21.0, 25.0];
        for (pair, reason) in [
            ((30.0, 10.0), "low above high"),
            ((f64::NAN, 80.0), "not finite"),
            ((-50.0, f64::NAN), "not finite"),
            ((-50.0, f64::INFINITY), "not finite"),
        ] {
            let mut action_bounds = bounds(&layout);
            action_bounds[1] = pair;
            let err = map_action_to_signals(&action, &layout, &sig_map, &action_bounds)
                .expect_err("a bad pair");
            assert!(err.contains("HVAC.cooling_setpoint_c"), "{err}");
            assert!(err.contains(reason), "{pair:?}: {err}");
        }
    }

    #[test]
    fn build_thermal_setpoint_rejects_a_deadband_without_a_setpoint() {
        let mut values = HashMap::new();
        values.insert("deadband_c".to_string(), 1.0);
        let err = build_control_signal("ThermalSetpoint", &values).unwrap_err();
        assert!(err.contains("names no setpoint"), "{err}");
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
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
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
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
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
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
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
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
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
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
        assert!(err.contains("must match action_layout length"));
    }

    #[test]
    fn map_missing_signal_type_errors() {
        let layout = vec![("UnknownEquip".to_string(), "fraction".to_string())];
        let sig_map = HashMap::new();
        let action = vec![0.5];
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
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
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
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
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
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
        let (signals, _clamped_count) =
            map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap();
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
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
        assert!(err.contains("not finite"));
    }

    #[test]
    fn map_action_rejects_infinity() {
        let (layout, sig_map) = therm_layout();
        let action = vec![f64::INFINITY, 26.0];
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
        assert!(err.contains("not finite"));
    }

    #[test]
    fn map_action_rejects_negative_infinity() {
        let (layout, sig_map) = therm_layout();
        let action = vec![21.0, f64::NEG_INFINITY];
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
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
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
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
        let err = map_action_to_signals(&action, &layout, &sig_map, &bounds(&layout)).unwrap_err();
        assert!(
            err.contains("not finite"),
            "error should mention not finite, got: {err}"
        );
    }

    /// Hand-run measurement (ignored; `cargo test -p hares-python --all-features
    /// --release --lib batch_step_allocation_counts -- --ignored --nocapture`):
    /// the counting allocator's per-call allocation delta for whole
    /// `batch_step` calls at 1 and 8 environments, 20 calls each after 10
    /// warm ones. The count is the calling thread's own: the parallel
    /// section's worker-thread allocations are not in it, so the 8-environment
    /// figure covers the GIL-held conversion section, and the per-environment
    /// observation read's own count is pinned exactly by hares-core's
    /// `observe_into_allocates_nothing_after_the_first_step`.
    #[test]
    #[ignore]
    fn batch_step_allocation_counts() {
        use hares_types::alloc_count::thread_allocations;

        Python::attach(|py| {
            let mut signal_types = HashMap::new();
            signal_types.insert("Gas Furnace".to_string(), "ThermalSetpoint".to_string());
            let layout = vec![("Gas Furnace".to_string(), "heating_setpoint_c".to_string())];
            let bounds_pair = vec![(-50.0, 80.0)];

            for n_envs in [1usize, 8] {
                let dwellings: Vec<Py<PyDwelling>> =
                    (0..n_envs).map(|_| base_dwelling(py)).collect();
                let plans: Vec<Py<PyObservationPlan>> = dwellings
                    .iter()
                    .map(|dwelling| {
                        let plan = dwelling
                            .bind(py)
                            .borrow()
                            .observation_plan(vec!["total_power_kw".to_string()])
                            .expect("the plan builds");
                        Py::new(py, plan).expect("the plan wraps")
                    })
                    .collect();
                let buffer = numpy::PyArray2::<f64>::zeros(py, (n_envs, 1), false);

                let mut deltas: Vec<u64> = Vec::new();
                for call in 0..30 {
                    let before = thread_allocations().expect("the counting allocator");
                    batch_step_py(
                        py,
                        dwellings
                            .iter()
                            .map(|dwelling| dwelling.clone_ref(py))
                            .collect(),
                        vec![vec![21.0]; n_envs],
                        plans.iter().map(|plan| plan.clone_ref(py)).collect(),
                        layout.clone(),
                        signal_types.clone(),
                        bounds_pair.clone(),
                        buffer.as_any(),
                    )
                    .expect("the batch step runs");
                    let after = thread_allocations().expect("the counting allocator");
                    if call >= 10 {
                        deltas.push(after - before);
                    }
                }
                deltas.sort_unstable();
                let median = deltas[deltas.len() / 2];
                println!(
                    "batch_step n_envs={n_envs}: per-call calling-thread allocations \
                     median {median}, min {}, max {} over {} calls",
                    deltas[0],
                    deltas[deltas.len() - 1],
                    deltas.len()
                );
            }
        });
    }
}
