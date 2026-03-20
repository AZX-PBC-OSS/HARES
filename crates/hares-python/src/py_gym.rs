//! Python bindings for Gymnasium RL environment.

use std::collections::HashMap;

use pyo3::Python;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rayon::prelude::*;

use crate::py_dwelling::PyDwelling;

/// One batched RL step result.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StepResult {
    pub obs: Vec<f64>,
    pub reward: f64,
    pub terminated: bool,
    pub truncated: bool,
    pub info: HashMap<String, f64>,
}

/// Parallel batch stepping entry point used by vectorized gym environments.
#[allow(dead_code)]
pub fn batch_step(dwellings: &mut [PyDwelling], actions: &[Vec<f64>]) -> Vec<StepResult> {
    dwellings
        .par_iter_mut()
        .enumerate()
        .map(|(idx, dwelling)| {
            // v1 baseline: actions are accepted for API shape but not yet mapped
            // into equipment-specific controls.
            let _ = actions.get(idx);

            let mut info = HashMap::new();
            match dwelling.step_core() {
                Ok(step) => {
                    let obs = dwelling.observation().unwrap_or_default();
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
                    }
                }
                Err(_) => StepResult {
                    obs: Vec::new(),
                    reward: 0.0,
                    terminated: true,
                    truncated: false,
                    info,
                },
            }
        })
        .collect()
}

/// GIL-releasing wrapper for batch stepping.
#[allow(dead_code)]
pub fn batch_step_with_py(
    py: Python<'_>,
    dwellings: &mut [PyDwelling],
    actions: &[Vec<f64>],
) -> Vec<StepResult> {
    py.detach(|| batch_step(dwellings, actions))
}

fn observation_for_fields(dwelling: &PyDwelling, fields: &[String]) -> Result<Vec<f64>, String> {
    let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
    let dwelling = dwelling
        .dwelling
        .lock()
        .map_err(|_| "failed to lock dwelling state".to_string())?;
    dwelling
        .telemetry()
        .to_observation_vec(&refs)
        .map_err(|err| err.to_string())
}

/// Python-exposed batched step entrypoint.
#[pyfunction(name = "batch_step")]
#[pyo3(signature = (dwellings, actions, observation_fields))]
pub fn batch_step_py(
    py: Python<'_>,
    dwellings: Vec<Py<PyDwelling>>,
    actions: Vec<Vec<f64>>,
    observation_fields: Vec<String>,
) -> PyResult<Vec<Py<PyAny>>> {
    let results = py.detach(|| {
        dwellings
            .par_iter()
            .enumerate()
            .map(|(idx, dwelling_obj)| {
                Python::attach(|py| {
                    let dwelling_ref = dwelling_obj.bind(py).borrow();
                    let _ = actions.get(idx);

                    let mut info = HashMap::new();
                    match dwelling_ref.step_core() {
                        Ok(step) => {
                            let obs = observation_for_fields(&dwelling_ref, &observation_fields)
                                .unwrap_or_else(|_| dwelling_ref.observation().unwrap_or_default());
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
                            }
                        }
                        Err(_) => StepResult {
                            obs: Vec::new(),
                            reward: 0.0,
                            terminated: true,
                            truncated: false,
                            info,
                        },
                    }
                })
            })
            .collect::<Vec<_>>()
    });

    let mut out = Vec::with_capacity(results.len());
    for item in results {
        let d = PyDict::new(py);
        d.set_item("obs", item.obs)?;
        d.set_item("reward", item.reward)?;
        d.set_item("terminated", item.terminated)?;
        d.set_item("truncated", item.truncated)?;
        d.set_item("info", item.info)?;
        out.push(d.unbind().into());
    }

    Ok(out)
}
