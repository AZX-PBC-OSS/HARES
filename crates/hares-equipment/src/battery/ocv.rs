//! Open-circuit voltage and negative electrode potential tables.

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// OCV table (open-circuit voltage vs SOC)
// ---------------------------------------------------------------------------

/// Table-based open-circuit voltage curve indexed by SOC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OcvTable {
    pub soc_points: Vec<f64>,
    pub voltage_v: Vec<f64>,
}

impl OcvTable {
    /// 11-point Li-NMC OCV curve (per cell).
    /// SOC 0.0 .. 1.0 in 0.1 steps. Calibrated values from NREL SSC / OCHRE.
    pub fn default_li_nmc() -> Self {
        Self {
            soc_points: vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            voltage_v: vec![
                3.0000, 3.4679, 3.5394, 3.5950, 3.6453, 3.6876, 3.7469, 3.8400, 3.9521, 4.0668,
                4.1934,
            ],
        }
    }

    /// Linear interpolation of cell OCV at a given SOC, clamped to table bounds.
    pub fn voltage_at_soc(&self, soc: f64) -> f64 {
        // SAFETY: soc_points and voltage_v are non-empty by construction (hardcoded LUT).
        let soc = soc.clamp(
            self.soc_points[0],
            *self.soc_points.last().expect("non-empty LUT"),
        );
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
        *self.voltage_v.last().expect("non-empty LUT")
    }

    pub fn new(soc_points: Vec<f64>, voltage_v: Vec<f64>) -> crate::Result<Self> {
        if soc_points.len() != voltage_v.len() {
            return Err(HaresError::Equipment(format!(
                "OCV table soc_points and voltage_v must have same length, got {} and {}",
                soc_points.len(),
                voltage_v.len()
            )));
        }
        if soc_points.is_empty() {
            return Err(HaresError::Equipment(
                "OCV table cannot be empty".to_string(),
            ));
        }
        for i in 1..soc_points.len() {
            if soc_points[i] <= soc_points[i - 1] {
                return Err(HaresError::Equipment(format!(
                    "OCV table soc_points must be strictly increasing, got {:?}",
                    soc_points
                )));
            }
        }
        for v in &voltage_v {
            if *v <= 0.0 {
                return Err(HaresError::Equipment(format!(
                    "OCV table voltage_v must be positive, got {:?}",
                    voltage_v
                )));
            }
        }
        Ok(Self {
            soc_points,
            voltage_v,
        })
    }
}

// ---------------------------------------------------------------------------
// Negative electrode potential table (Smith 2017 degradation model)
// ---------------------------------------------------------------------------

/// Table-based negative electrode half-cell potential curve indexed by SOC.
/// Used by the Smith 2017 SEI Tafel correction in `DegradationState`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UNegTable {
    pub soc_points: Vec<f64>,
    pub potential_v: Vec<f64>,
}

impl UNegTable {
    /// 11-point Li-NMC U_neg curve (graphite anode, vs Li/Li+).
    /// Values from NREL SSC / OCHRE Smith 2017 calibration.
    pub fn default_li_nmc() -> Self {
        Self {
            soc_points: vec![0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
            potential_v: vec![
                1.2868, 0.2420, 0.1818, 0.1488, 0.1297, 0.1230, 0.1181, 0.1061, 0.0925, 0.0876,
                0.0859,
            ],
        }
    }

    /// Linear interpolation of U_neg at a given SOC, clamped to table bounds.
    pub fn potential_at_soc(&self, soc: f64) -> f64 {
        // SAFETY: soc_points and potential_v are non-empty by construction (hardcoded LUT).
        let soc = soc.clamp(
            self.soc_points[0],
            *self.soc_points.last().expect("non-empty LUT"),
        );
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
        *self.potential_v.last().expect("non-empty LUT")
    }

    pub fn new(soc_points: Vec<f64>, potential_v: Vec<f64>) -> crate::Result<Self> {
        if soc_points.len() != potential_v.len() {
            return Err(HaresError::Equipment(format!(
                "UNeg table soc_points and potential_v must have same length, got {} and {}",
                soc_points.len(),
                potential_v.len()
            )));
        }
        if soc_points.is_empty() {
            return Err(HaresError::Equipment(
                "UNeg table cannot be empty".to_string(),
            ));
        }
        for i in 1..soc_points.len() {
            if soc_points[i] <= soc_points[i - 1] {
                return Err(HaresError::Equipment(format!(
                    "UNeg table soc_points must be strictly increasing, got {:?}",
                    soc_points
                )));
            }
        }
        for v in &potential_v {
            if !v.is_finite() || v.is_nan() {
                return Err(HaresError::Equipment(format!(
                    "UNeg table potential_v must be finite, got {:?}",
                    potential_v
                )));
            }
        }
        Ok(Self {
            soc_points,
            potential_v,
        })
    }
}
