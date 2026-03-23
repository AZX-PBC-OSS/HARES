//! Port-to-input-vector aggregation for the thermal solver.
//!
//! Reads equipment thermal contributions from [`PortSlots`] and writes the
//! per-zone sensible gains into the state-space input vector.

use hares_types::PortSlots;
use nalgebra::DVector;

use super::ThermalSolver;

impl ThermalSolver {
    /// Accumulates per-zone sensible heat gains from equipment ports into the
    /// input vector at the corresponding zone sensible input indices.
    pub(super) fn apply_port_sensible_inputs(&self, u: &mut DVector<f64>, ports: &PortSlots) {
        for thermal in &ports.thermal {
            if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&thermal.zone)
                && idx < u.len()
            {
                u[idx] += thermal.sensible_gain_w;
            }
        }
    }
}
