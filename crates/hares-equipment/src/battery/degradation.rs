//! Battery degradation model (Smith et al. 2017, IEEE 7963578) and rainflow cycle counter.

use serde::{Deserialize, Serialize};

use super::ocv::UNegTable;
use super::SECONDS_PER_DAY;

// ---------------------------------------------------------------------------
// Rainflow cycle counter (ASTM E1049-85 simplified)
// ---------------------------------------------------------------------------

/// Simplified rainflow half-cycle counter for battery degradation.
///
/// Tracks SOC reversals and counts completed cycles using the 3-point method.
/// Per-cycle DOD amplitudes are stored separately so the degradation model can
/// compute `Σ(DOD_i)` and `Σ(DOD_i²)` terms required by Smith 2017.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct RainflowCounter {
    /// SOC values at detected reversals (peaks and valleys).
    pub(crate) reversals: Vec<f64>,
    /// Accumulated equivalent full cycle count (sum of cycle weights).
    cycle_count: f64,
    /// DOD amplitudes of cycles accumulated within the current day.
    /// Each entry is `range * weight` (weight = 0.5 for half-cycles, 1.0 for full).
    daily_cycle_dods: Vec<f64>,
}

impl RainflowCounter {
    pub(crate) fn push(&mut self, soc: f64) {
        let n = self.reversals.len();
        if n < 2 {
            // Need at least two points to detect a reversal direction.
            if n == 0 || (self.reversals[n - 1] - soc).abs() > f64::EPSILON {
                self.reversals.push(soc);
            }
            return;
        }
        let last = self.reversals[n - 1];
        let prev = self.reversals[n - 2];
        let prev_dir = last - prev; // positive = rising, negative = falling
        let cur_dir = soc - last;

        // If direction hasn't changed, update the last point (extend the current ramp).
        if prev_dir * cur_dir >= 0.0 {
            *self.reversals.last_mut().unwrap() = soc;
        } else {
            // Direction reversed -- record the reversal and try to extract cycles.
            self.reversals.push(soc);
            self.extract_cycles();
        }
    }

    /// Rainflow cycle extraction per ASTM E1049-85, matching the Python `rainflow`
    /// library (v3.2) used by OCHRE.
    ///
    /// Uses the 3 most recent reversal points to form ranges X and Y:
    ///   x1 = points[-3], x2 = points[-2], x3 = points[-1]
    ///   X = |x3 - x2|  (newest range)
    ///   Y = |x2 - x1|  (middle range)
    ///
    /// Extract when X >= Y. If only 3 points remain, count Y as a half-cycle
    /// and remove the first point. If 4+ points remain, count Y as a full cycle
    /// and remove the valley and peak of Y (the two middle points).
    fn extract_cycles(&mut self) {
        loop {
            let n = self.reversals.len();
            if n < 3 {
                break;
            }
            let x1 = self.reversals[n - 3];
            let x2 = self.reversals[n - 2];
            let x3 = self.reversals[n - 1];

            let range_x = (x3 - x2).abs(); // newest range
            let range_y = (x2 - x1).abs(); // middle range

            if range_x < range_y {
                break;
            }

            if n == 3 {
                // Y contains the starting point: count as half-cycle, discard first point.
                let weight = 0.5;
                self.cycle_count += range_y * weight;
                self.daily_cycle_dods.push(range_y * weight);
                self.reversals.remove(0);
            } else {
                // Count Y as a full cycle, remove the peak and valley of Y.
                self.cycle_count += range_y;
                self.daily_cycle_dods.push(range_y);
                // Remove points at n-3 and n-2 (the middle two of the 4-point window).
                let last = self.reversals.pop().unwrap();
                self.reversals.pop(); // was n-2
                self.reversals.pop(); // was n-3
                self.reversals.push(last);
            }
        }
    }

    pub(crate) fn total_cycles(&self) -> f64 {
        self.cycle_count
    }

    /// Sum of squared effective DOD values accumulated today (Σ DOD_i²).
    /// Used by the Smith 2017 cycle-aging mechanism (b2 term).
    pub(crate) fn sum_squared_dod_daily(&self) -> f64 {
        self.daily_cycle_dods.iter().map(|d| d * d).sum()
    }

    pub(crate) fn reset_daily(&mut self) {
        // Keep the full reversal buffer. Residential charge/discharge cycles can
        // straddle midnight, so discarding the buffer would lose partial cycles.
        // Clear daily DOD list; it is consumed by the degradation model at midnight.
        self.daily_cycle_dods.clear();
    }
}

// ---------------------------------------------------------------------------
// Degradation model (Smith et al. 2017, IEEE 7963578)
// ---------------------------------------------------------------------------

/// Physical constants shared by the degradation model.
mod deg_const {
    pub const R_GAS: f64 = 8.314; // J/(K·mol)
    pub const F_FARADAY: f64 = 96_485.0; // A·s/mol
    pub const T_REF: f64 = 298.15; // K (25 °C)
    pub const V_REF: f64 = 3.7; // V reference OCV
    pub const U_NEG_REF: f64 = 0.08; // V reference negative electrode potential

    // Mechanism 1: Calendar / SEI growth (sqrt-of-time)
    pub const B1_REF: f64 = 3.503e-3; // day^-0.5
    pub const EA_B1: f64 = 35_392.0; // J/mol
    pub const ALPHA_B1: f64 = -1.0; // Tafel symmetry factor
    pub const BETA_B1: f64 = 2.157; // DOD power-law exponent
    pub const GAMMA_B1: f64 = 2.472; // DOD coupling coefficient

    // Mechanism 2: Cycle aging
    pub const B2_REF: f64 = 1.541e-5; // cycle^-1
    pub const EA_B2: f64 = -42_800.0; // J/mol (negative ⟹ faster at lower T)

    // Mechanism 3: BOL transient / early-life lithium loss
    pub const B3_REF: f64 = -2.805e-2; // dimensionless
    pub const EA_B3: f64 = 42_800.0; // J/mol
    pub const ALPHA_B3: f64 = 0.0066; // Tafel factor for V_oc
    pub const TAU_B3: f64 = 5.0; // days
    pub const THETA: f64 = -0.135; // DOD coupling

    // Initial lithium inventory (OCHRE value)
    pub const B0: f64 = 1.0;
}

/// Full Smith 2017 (IEEE 7963578) three-mechanism battery degradation model.
///
/// Three lithium-loss mechanisms:
///   `q_li1` — calendar SEI growth (sqrt-of-time, Arrhenius + Tafel + DOD)
///   `q_li2` — cycle-induced lithium loss (Arrhenius + rainflow DOD² sum)
///   `q_li3` — beginning-of-life transient (exponential decay)
///
/// Capacity fade = 1 − (b0 − q_li1 − q_li2 − q_li3).clamp(0, ∞)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DegradationState {
    // ---- Per-timestep accumulators (reset each day) ----
    /// Σ b1_ref · exp(−Ea_b1/R · (1/T − 1/T_ref)) · dt_day
    pub(crate) b1_accum: f64,
    /// Σ exp(−Ea_b2/R · (1/T − 1/T_ref)) · dt_day
    b2_accum: f64,
    /// Σ b3_ref · exp(−Ea_b3/R · (1/T − 1/T_ref)) · exp(α_b3·F/R · (V_oc/T − V_ref/T_ref)) · (1 + θ·DOD) · dt_day
    b3_accum: f64,

    // ---- Cumulative lithium losses ----
    q_li1: f64,
    q_li2: f64,
    pub(crate) q_li3: f64,

    // ---- Day-level tracking ----
    /// Age of the cell in whole days (incremented at each midnight update).
    day_age: u32,
    /// Maximum DOD seen during the current day [0..1] = max_soc_today − min_soc_today.
    dod_max_today: f64,
    /// Peak SOC seen today (for computing DOD = peak − trough).
    soc_max_today: f64,
    /// Trough SOC seen today (for computing DOD = peak − trough).
    soc_min_today: f64,
    /// SOC at the trough point when max DOD was reached (used for U_neg Tafel correction).
    soc_at_max_dod: f64,
    /// Computed capacity fade fraction [0..1].
    capacity_fade: f64,
}

impl Default for DegradationState {
    fn default() -> Self {
        Self {
            b1_accum: 0.0,
            b2_accum: 0.0,
            b3_accum: 0.0,
            q_li1: 0.0,
            q_li2: 0.0,
            q_li3: 0.0,
            day_age: 0,
            dod_max_today: 0.0,
            soc_max_today: 0.5,
            soc_min_today: 0.5,
            soc_at_max_dod: 0.5,
            capacity_fade: 0.0,
        }
    }
}

impl DegradationState {
    /// Capacity fade fraction in [0, 1].  0.0 = no fade (new cell); 1.0 = fully degraded.
    pub(crate) fn capacity_fade_pct(&self) -> f64 {
        self.capacity_fade
    }

    /// Called every simulation timestep to accumulate sub-daily degradation terms.
    ///
    /// `dt_s`        — timestep in seconds
    /// `cell_temp_k` — cell temperature in kelvin
    /// `v_oc`        — cell open-circuit voltage at current SOC (V)
    /// `soc`         — current state of charge [0..1]
    pub(crate) fn accumulate(&mut self, dt_s: f64, cell_temp_k: f64, v_oc: f64, soc: f64) {
        use deg_const::*;
        let dt_day = dt_s / SECONDS_PER_DAY;
        let t = cell_temp_k;
        let inv_diff = 1.0 / t - 1.0 / T_REF;

        // Update daily SOC extremes for DOD computation.
        if soc > self.soc_max_today {
            self.soc_max_today = soc;
        }
        if soc < self.soc_min_today {
            self.soc_min_today = soc;
        }
        let dod = (self.soc_max_today - self.soc_min_today).max(0.0);
        if dod > self.dod_max_today {
            self.dod_max_today = dod;
            // Record the trough SOC for Tafel U_neg lookup.
            self.soc_at_max_dod = self.soc_min_today;
        }

        // Mechanism 1: calendar (Arrhenius only; Tafel+DOD corrections are applied daily)
        self.b1_accum += B1_REF * (-(EA_B1 / R_GAS) * inv_diff).exp() * dt_day;

        // Mechanism 2: cycle (Arrhenius temperature factor)
        self.b2_accum += (-(EA_B2 / R_GAS) * inv_diff).exp() * dt_day;

        // Mechanism 3: BOL transient — use running dod_max_today as best estimate
        let tafel_b3 = ((ALPHA_B3 * F_FARADAY / R_GAS) * (v_oc / t - V_REF / T_REF)).exp();
        self.b3_accum +=
            B3_REF * (-(EA_B3 / R_GAS) * inv_diff).exp() * tafel_b3 * (1.0 + THETA * dod) * dt_day;
    }

    /// Reset daily tracking fields at the start of each new day.
    /// Called after `update_daily()` to prepare for the next 24-hour window.
    pub(crate) fn reset_day_tracking(&mut self, soc: f64) {
        self.soc_max_today = soc;
        self.soc_min_today = soc;
        self.dod_max_today = 0.0;
        self.soc_at_max_dod = soc;
    }

    /// Called once per day (at midnight) to compute lithium-loss increments and
    /// update the cumulative capacity fade.
    ///
    /// `u_neg_table`        — negative electrode potential lookup table
    /// `cell_temp_k`        — representative cell temperature for the day (K)
    /// `sum_squared_dod`    — Σ DOD_i² from today's rainflow cycles
    pub(crate) fn update_daily(&mut self, u_neg_table: &UNegTable, cell_temp_k: f64, sum_squared_dod: f64) {
        use deg_const::*;

        let t_day = cell_temp_k;
        let dod_max = self.dod_max_today;

        // ---- Step 1: Tafel and DOD corrections for mechanism 1 ----
        let u_neg = u_neg_table.potential_at_soc(self.soc_at_max_dod);
        let tafel_b1 = ((ALPHA_B1 * F_FARADAY / R_GAS) * (u_neg / t_day - U_NEG_REF / T_REF)).exp();
        let b1_eff = self.b1_accum * tafel_b1 * (GAMMA_B1 * dod_max.powf(BETA_B1)).exp();

        // ---- Step 2: Three lithium-loss increments ----

        // Mechanism 1: calendar SEI (sqrt-of-time progression)
        let dq_li1 = if self.q_li1.abs() < 1e-5 && self.day_age > 0 {
            b1_eff / (self.day_age as f64).sqrt()
        } else if self.q_li1.abs() >= 1e-5 {
            0.5 * b1_eff.powi(2) / self.q_li1
        } else {
            // day_age == 0: first day, skip to avoid division-by-zero
            0.0
        };

        // Mechanism 2: cycle aging
        let dq_li2 = B2_REF * self.b2_accum * sum_squared_dod.sqrt();

        // Mechanism 3: BOL transient (exponential relaxation toward b3_accum)
        let dq_li3 = (self.b3_accum - self.q_li3).max(0.0) / TAU_B3;

        // ---- Step 3: Update cumulative lithium losses ----
        self.q_li1 += dq_li1;
        self.q_li2 += dq_li2;
        self.q_li3 += dq_li3;

        // ---- Step 4: Capacity fade fraction ----
        let remaining = (B0 - self.q_li1 - self.q_li2 - self.q_li3).max(0.0);
        self.capacity_fade = 1.0 - remaining;

        // ---- Reset accumulators for next day ----
        self.b1_accum = 0.0;
        self.b2_accum = 0.0;
        self.b3_accum = 0.0;
        self.day_age += 1;
        // soc extremes and dod_max are reset by the caller via reset_day_tracking().
    }
}
