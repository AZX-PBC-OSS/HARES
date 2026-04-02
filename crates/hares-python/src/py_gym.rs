//! Python bindings for Gymnasium RL environment.

use std::collections::HashMap;

use pyo3::Python;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rayon::prelude::*;

use crate::py_dwelling::{PyDwelling, lock_dwelling_string};

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
    let dwelling = lock_dwelling_string(&dwelling.dwelling)?;
    dwelling
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

/// Python-exposed batched step entrypoint.
///
/// Binds all dwelling handles while holding the GIL, extracts raw pointers,
/// then releases the GIL so Rayon threads can step dwellings in true parallel.
/// The GIL is only re-acquired afterwards for Python dict construction.
#[pyfunction(name = "batch_step")]
#[pyo3(signature = (dwellings, actions, observation_fields))]
pub fn batch_step_py(
    py: Python<'_>,
    dwellings: Vec<Py<PyDwelling>>,
    actions: Vec<Vec<f64>>,
    observation_fields: Vec<String>,
) -> PyResult<Vec<Py<PyAny>>> {
    // Dimension check first (useful error regardless of H-7 status).
    if !actions.is_empty() && actions.len() != dwellings.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "actions length ({}) must match dwellings length ({})",
            actions.len(),
            dwellings.len(),
        )));
    }
    // Action mapping is not yet implemented. Reject non-empty actions
    // at the Python boundary with a clear error before entering Rayon.
    if actions.iter().any(|a| !a.is_empty()) {
        return Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "batch_step action mapping is not yet implemented (see H-7). \
             Apply controls via dwelling.apply_control() before calling batch_step, \
             and pass empty action vectors.",
        ));
    }

    // Hold borrows alive for the duration of the parallel section.
    // The Py<PyDwelling> vec keeps every object alive for the entire function.
    let borrows: Vec<PyRef<'_, PyDwelling>> =
        dwellings.iter().map(|d| d.bind(py).borrow()).collect();
    let ptrs: Vec<SendDwellingPtr> = borrows
        .iter()
        .map(|b| SendDwellingPtr(&**b as *const PyDwelling))
        .collect();
    // borrows live until end of function scope

    // Release GIL — Rayon threads run step_core()/observation() without
    // touching Python. Both methods use only Mutex<Dwelling> internally.
    let results: Vec<StepResult> = py.detach(|| {
        ptrs.par_iter()
            .enumerate()
            .map(|(_idx, SendDwellingPtr(ptr))| {
                // SAFETY: the PyRef borrows in `borrows` and Py<PyDwelling> handles keep the
                // objects alive. step_core() and observation() are GIL-free
                // (they only lock the internal Mutex<Dwelling>).
                let dwelling = unsafe { &**ptr };

                let mut info = HashMap::new();
                let step_result = std::panic::catch_unwind(
                    std::panic::AssertUnwindSafe(|| dwelling.step_core_string()),
                );
                let step_result = match step_result {
                    Ok(inner) => inner,
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic");
                        Err(format!("HARES internal panic: {msg}"))
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
                        info.insert("error".to_string(), 1.0);
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

    // GIL re-acquired here — convert results to Python dicts.
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
