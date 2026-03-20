//! Open-circuit voltage and negative electrode potential tables.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// OCV table (open-circuit voltage vs SOC)
// ---------------------------------------------------------------------------

/// Table-based open-circuit voltage curve indexed by SOC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct OcvTable {
    soc_points: Vec<f64>,
    voltage_v: Vec<f64>,
}

impl OcvTable {
    /// 11-point Li-NMC OCV curve (per cell).
    /// SOC 0.0 .. 1.0 in 0.1 steps. Calibrated values from NREL SSC / OCHRE.
    pub(crate) fn default_li_nmc() -> Self {
        Self {
            soc_points: vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            voltage_v: vec![
                3.0000, 3.4679, 3.5394, 3.5950, 3.6453, 3.6876, 3.7469, 3.8400, 3.9521, 4.0668,
                4.1934,
            ],
        }
    }

    /// Linear interpolation of cell OCV at a given SOC, clamped to table bounds.
    pub(crate) fn voltage_at_soc(&self, soc: f64) -> f64 {
        let soc = soc.clamp(self.soc_points[0], *self.soc_points.last().unwrap());
        // Find bracketing interval
        let n = self.soc_points.len();
        if n == 1 {
            return self.voltage_v[0];
        }
        for i in 0..n - 1 {
            if soc <= self.soc_points[i + 1] {
                let span = self.soc_points[i + 1] - self.soc_points[i];
                if span.abs() < f64::EPSILON {
                    return self.voltage_v[i];
                }
                let t = (soc - self.soc_points[i]) / span;
                return self.voltage_v[i] + t * (self.voltage_v[i + 1] - self.voltage_v[i]);
            }
        }
        *self.voltage_v.last().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Negative electrode potential table (Smith 2017 degradation model)
// ---------------------------------------------------------------------------

/// Table-based negative electrode half-cell potential curve indexed by SOC.
/// Used by the Smith 2017 SEI Tafel correction in `DegradationState`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct UNegTable {
    soc_points: Vec<f64>,
    potential_v: Vec<f64>,
}

impl UNegTable {
    /// 11-point Li-NMC U_neg curve (graphite anode, vs Li/Li+).
    /// Values from NREL SSC / OCHRE Smith 2017 calibration.
    pub(crate) fn default_li_nmc() -> Self {
        Self {
            soc_points: vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            potential_v: vec![
                1.2868, 0.2420, 0.1818, 0.1488, 0.1297, 0.1230, 0.1181, 0.1061, 0.0925, 0.0876,
                0.0859,
            ],
        }
    }

    /// Linear interpolation of U_neg at a given SOC, clamped to table bounds.
    pub(crate) fn potential_at_soc(&self, soc: f64) -> f64 {
        let soc = soc.clamp(self.soc_points[0], *self.soc_points.last().unwrap());
        let n = self.soc_points.len();
        if n == 1 {
            return self.potential_v[0];
        }
        for i in 0..n - 1 {
            if soc <= self.soc_points[i + 1] {
                let span = self.soc_points[i + 1] - self.soc_points[i];
                if span.abs() < f64::EPSILON {
                    return self.potential_v[i];
                }
                let t = (soc - self.soc_points[i]) / span;
                return self.potential_v[i] + t * (self.potential_v[i + 1] - self.potential_v[i]);
            }
        }
        *self.potential_v.last().unwrap()
    }
}
