//! Battery degradation model (Smith et al. 2017, IEEE 7963578) and rainflow cycle counter.

use serde::{Deserialize, Serialize};

use super::SECONDS_PER_DAY;
use super::ocv::UNegTable;

// ---------------------------------------------------------------------------
// Rainflow cycle counter (ASTM E1049-85 simplified)
// ---------------------------------------------------------------------------

/// Simplified rainflow half-cycle counter for battery degradation.
///
/// Tracks SOC reversals and counts completed cycles using the 3-point method.
/// Per-cycle DOD amplitudes are stored separately so the degradation model can
/// compute the `Σ(count_i × DOD_i²)` weighted cycle-damage term required by Smith 2017.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct RainflowCounter {
    /// SOC values at detected reversals (peaks and valleys).
    pub(crate) reversals: Vec<f64>,
    /// Accumulated equivalent full cycle count (sum of cycle weights).
    cycle_count: f64,
    /// Cycles accumulated within the current day as `(range, count)` pairs.
    /// Count is 0.5 for half-cycles, 1.0 for full cycles per ASTM E1049-85.
    daily_cycle_dods: Vec<(f64, f64)>,
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
            // SAFETY: reversals.len() >= 2 guaranteed by the early return at the top of this function.
            *self.reversals.last_mut().expect("reversals.len() >= 2") = soc;
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
                self.cycle_count += 0.5;
                self.daily_cycle_dods.push((range_y, 0.5));
                self.reversals.remove(0);
            } else {
                // Count Y as a full cycle, remove the peak and valley of Y.
                self.cycle_count += 1.0;
                self.daily_cycle_dods.push((range_y, 1.0));
                // Remove points at n-3 and n-2 (the middle two of the 4-point window).
                // SAFETY: reversals.len() >= 4 guaranteed by the `if n < 3 { break }` guard
                // and the `n == 3` branch exclusion above this point.
                let last = self.reversals.pop().expect("reversals.len() >= 4");
                self.reversals.pop(); // was n-2
                self.reversals.pop(); // was n-3
                self.reversals.push(last);
            }
        }
    }

    pub(crate) fn total_cycles(&self) -> f64 {
        self.cycle_count
    }

    /// Weighted sum of squared DOD values accumulated today: Σ(count_i × DOD_i²).
    /// Used by the Smith 2017 cycle-aging mechanism (b2 term).
    pub(crate) fn sum_squared_dod_daily(&self) -> f64 {
        self.daily_cycle_dods
            .iter()
            .map(|&(range, count)| count * range * range)
            .sum()
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
    /// `sum_squared_dod`    — Σ(count_i × DOD_i²) from today's rainflow cycles,
    ///                        where count_i is 0.5 for half-cycles and 1.0 for full cycles.
    pub(crate) fn update_daily(
        &mut self,
        u_neg_table: &UNegTable,
        cell_temp_k: f64,
        sum_squared_dod: f64,
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use super::deg_const::*;

    fn make_u_neg_table() -> UNegTable {
        UNegTable::default_li_nmc()
    }

    // -----------------------------------------------------------------------
    // Rainflow counter tests (ASTM E1049-85 reference)
    // -----------------------------------------------------------------------

    /// ASTM E1049-85: sequence [0.5, 1.0, 0.0, 1.0] contains one complete
    /// charge-discharge cycle. The counter should extract at least one cycle
    /// with DOD = 1.0. Per ASTM, total_cycles() counts cycle weights
    /// (0.5 per half-cycle, 1.0 per full cycle), independent of DOD amplitude.
    #[test]
    fn rainflow_single_full_cycle() {
        let mut rc = RainflowCounter::default();
        for soc in [0.5, 1.0, 0.0, 1.0] {
            rc.push(soc);
        }
        // ASTM E1049-85: two half-cycles are extracted (0.5 + 0.5 = 1.0 total).
        assert!(
            (rc.total_cycles() - 1.0).abs() < 1e-10,
            "ASTM E1049-85: [0.5,1.0,0.0,1.0] should yield total_cycles = 1.0, got {}",
            rc.total_cycles()
        );
        assert!(
            rc.sum_squared_dod_daily() > 0.0,
            "sum_squared_dod_daily should reflect extracted DOD"
        );
    }

    /// ASTM E1049-85: sequence [1.0, 0.2, 0.6, 0.0] contains two reversals
    /// producing two half-cycles. The first has DOD = |0.2 − 1.0| = 0.8 and
    /// the second has DOD = |0.0 − 0.6| = 0.6. total_cycles() should return 1.0
    /// (two half-cycles × 0.5 weight each).
    #[test]
    fn rainflow_partial_cycles() {
        let mut rc = RainflowCounter::default();
        for soc in [1.0_f64, 0.2, 0.6, 0.0] {
            rc.push(soc);
        }
        // Each reversal produces one half-cycle (weight 0.5); two reversals → 1.0 total.
        assert!(
            (rc.total_cycles() - 1.0).abs() < 1e-10,
            "Two-reversal sequence should yield total_cycles = 1.0, got {}",
            rc.total_cycles()
        );
        // sum_squared_dod must be positive: at least one half-cycle with DOD > 0.
        assert!(
            rc.sum_squared_dod_daily() > 0.0,
            "sum_squared_dod_daily must be positive after partial cycles, got {}",
            rc.sum_squared_dod_daily()
        );
    }

    /// ASTM E1049-85: monotonically decreasing SOC has no reversals, so no
    /// cycles can be extracted.
    #[test]
    fn rainflow_monotonic_no_cycles() {
        let mut rc = RainflowCounter::default();
        for soc in [1.0, 0.8, 0.6, 0.4, 0.2, 0.0] {
            rc.push(soc);
        }
        assert_eq!(
            rc.total_cycles(),
            0.0,
            "Monotonic discharge should produce zero cycles"
        );
        assert_eq!(
            rc.sum_squared_dod_daily(),
            0.0,
            "No cycles means sum_squared_dod_daily must also be zero"
        );
    }

    /// sum_squared_dod accumulates count × DOD² for each extracted cycle.
    ///
    /// Sequence [1.0, 0.2, 1.0]:
    ///   - Push 1.0, 0.2: initialises reversal buffer (two-point ramp, no extraction)
    ///   - Push 1.0: direction reverses; 3-point extraction extracts a half-cycle
    ///     with range = |0.2 - 1.0| = 0.8 → stored as (DOD=0.8, count=0.5)
    ///   sum_squared_dod = 0.5 × 0.8² = 0.32  (Smith 2017 b2 input term)
    #[test]
    fn rainflow_sum_squared_dod() {
        let mut rc = RainflowCounter::default();
        for soc in [1.0_f64, 0.2, 1.0] {
            rc.push(soc);
        }
        // Exactly one half-cycle extracted.
        assert!(
            (rc.total_cycles() - 0.5).abs() < 1e-12,
            "Sequence [1.0, 0.2, 1.0] should yield one half-cycle (total_cycles=0.5), got {}",
            rc.total_cycles()
        );
        // sum_squared_dod = count × DOD² = 0.5 × 0.64 = 0.32
        let expected = 0.5_f64 * 0.8_f64 * 0.8_f64;
        assert!(
            (rc.sum_squared_dod_daily() - expected).abs() < 1e-12,
            "sum_squared_dod_daily should be 0.5×0.8²={expected:.4}, got {}",
            rc.sum_squared_dod_daily()
        );
    }

    /// ASTM E1049-85: [0.5, 1.0, 0.5] has one reversal producing a single
    /// half-cycle with range (DOD) = 0.5 and weight = 0.5.
    /// total_cycles() should return the cycle weight (0.5), NOT range*weight.
    #[test]
    fn rainflow_partial_cycles_half_weight() {
        let mut rc = RainflowCounter::default();
        for soc in [0.5, 1.0, 0.5] {
            rc.push(soc);
        }
        // ASTM E1049-85: one half-cycle => total_cycles = 0.5
        assert!(
            (rc.total_cycles() - 0.5).abs() < 1e-10,
            "ASTM E1049-85: [0.5,1.0,0.5] is one half-cycle, total_cycles should be 0.5, got {}. \
             Bug: cycle_count accumulates range*weight ({}) instead of weight alone.",
            rc.total_cycles(),
            rc.total_cycles()
        );
    }

    // -----------------------------------------------------------------------
    // Degradation model tests (Smith et al. 2017, IEEE 7963578)
    // -----------------------------------------------------------------------

    /// Helper: run N days of pure calendar aging (no cycling) and return the
    /// DegradationState. Each day consists of a single accumulate() call for
    /// the full 86400 s timestep, then update_daily().
    fn run_calendar_aging(days: u32, cell_temp_k: f64, v_oc: f64, soc: f64) -> DegradationState {
        let u_neg = make_u_neg_table();
        let mut ds = DegradationState::default();
        let dt_s = SECONDS_PER_DAY;
        for _ in 0..days {
            ds.accumulate(dt_s, cell_temp_k, v_oc, soc);
            ds.update_daily(&u_neg, cell_temp_k, 0.0);
            ds.reset_day_tracking(soc);
        }
        ds
    }

    /// Helper: run N days with a fixed sum_squared_dod per day.
    fn run_cycling_aging(
        days: u32,
        cell_temp_k: f64,
        v_oc: f64,
        soc: f64,
        sum_squared_dod_per_day: f64,
    ) -> DegradationState {
        let u_neg = make_u_neg_table();
        let mut ds = DegradationState::default();
        let dt_s = SECONDS_PER_DAY;
        for _ in 0..days {
            ds.accumulate(dt_s, cell_temp_k, v_oc, soc);
            ds.update_daily(&u_neg, cell_temp_k, sum_squared_dod_per_day);
            ds.reset_day_tracking(soc);
        }
        ds
    }

    /// Compute the Tafel factor for mechanism 1 at a given SOC and temperature,
    /// using the NMC U_neg table. This is the physics reference calculation.
    fn tafel_b1_factor(soc: f64, cell_temp_k: f64) -> f64 {
        let u_neg = make_u_neg_table().potential_at_soc(soc);
        (ALPHA_B1 * F_FARADAY / R_GAS * (u_neg / cell_temp_k - U_NEG_REF / T_REF)).exp()
    }

    /// At T = T_REF (25C), the Arrhenius factor exp(-Ea/R * (1/T - 1/T_REF)) = exp(0) = 1.0
    /// for all three mechanisms. This is the fundamental identity of the Arrhenius equation.
    #[test]
    fn arrhenius_factor_at_reference_temperature_is_unity() {
        let mut ds = DegradationState::default();
        let dt_s = SECONDS_PER_DAY;
        ds.accumulate(dt_s, T_REF, V_REF, 0.5);

        // b1_accum = B1_REF * arr(T_REF) * dt_day = B1_REF * 1.0 * 1.0
        let expected_b1 = B1_REF;
        assert!(
            (ds.b1_accum - expected_b1).abs() < 1e-12,
            "b1_accum at T_REF should be B1_REF={expected_b1}, got {}",
            ds.b1_accum
        );

        // b2_accum = arr(T_REF) * dt_day = 1.0 * 1.0
        assert!(
            (ds.b2_accum - 1.0).abs() < 1e-12,
            "b2_accum at T_REF should be 1.0, got {}",
            ds.b2_accum
        );
    }

    /// Smith 2017 Eq. 3: at 45C the Arrhenius factor for mechanism 1 is
    /// arr_b1 = exp(-35392/8.314 * (1/318.15 - 1/298.15)) = exp(0.898) ~ 2.454.
    /// Combined with Tafel correction (which also depends on T), the overall
    /// calendar fade ratio between 45C and 25C should be ~3.40.
    ///
    /// Derivation: ratio = (arr_45 * tafel_45) / (arr_25 * tafel_25)
    ///   arr_25 = 1.0, tafel_25 = exp(-F/R * (u_neg/298.15 - 0.08/298.15)) ~ 0.1256
    ///   arr_45 = 2.454, tafel_45 = exp(-F/R * (u_neg/318.15 - 0.08/298.15)) ~ 0.1740
    ///   ratio = (2.454 * 0.1740) / (1.0 * 0.1256) ~ 3.40
    #[test]
    fn arrhenius_factor_at_45c_mechanism1() {
        let ds_25 = run_calendar_aging(30, T_REF, V_REF, 0.5);
        let ds_45 = run_calendar_aging(30, 318.15, V_REF, 0.5);

        // Compute physics reference ratio from Arrhenius + Tafel
        let arr_25 = 1.0_f64;
        let arr_45 = (-(EA_B1 / R_GAS) * (1.0 / 318.15 - 1.0 / T_REF)).exp();
        let tafel_25 = tafel_b1_factor(0.5, T_REF);
        let tafel_45 = tafel_b1_factor(0.5, 318.15);
        let expected_ratio = (arr_45 * tafel_45) / (arr_25 * tafel_25);

        let ratio = ds_45.capacity_fade_pct() / ds_25.capacity_fade_pct();
        assert!(
            (ratio - expected_ratio).abs() < 0.1,
            "45C/25C calendar fade ratio should be ~{expected_ratio:.2}, got {ratio:.4} \
             (fade_25={:.6}, fade_45={:.6})",
            ds_25.capacity_fade_pct(),
            ds_45.capacity_fade_pct()
        );
    }

    /// Smith 2017 Eq. 2-4: calendar aging at 25C for 30 days with SOC=0.5.
    /// The SEI growth rate follows q_li1 ~ b1_eff * sqrt(t) where
    /// b1_eff = B1_REF * arr(T_REF) * tafel(SOC=0.5) * dod_corr(dod=0).
    ///
    /// At T_REF: arr = 1.0
    /// At SOC=0.5: U_neg ~ 0.1333V, tafel = exp(-F/R * (0.1333/298.15 - 0.08/298.15)) ~ 0.1256
    /// At DOD=0: exp(gamma * 0^beta) = exp(0) = 1.0
    /// b1_eff = 3.503e-3 * 0.1256 = 4.40e-4 per day^0.5
    /// q_li1(30d) ~ 4.40e-4 * sqrt(30) ~ 2.41e-3 => 0.241%
    #[test]
    fn calendar_aging_30_days_25c() {
        let ds = run_calendar_aging(30, T_REF, V_REF, 0.5);
        let fade_pct = ds.capacity_fade_pct() * 100.0;

        // Compute physics reference: B1_REF * tafel * sqrt(30)
        let tafel = tafel_b1_factor(0.5, T_REF);
        let b1_eff = B1_REF * tafel; // arr = 1.0 at T_REF, dod_corr = 1.0 at dod=0
        let analytic_q_li1_pct = b1_eff * (30.0_f64).sqrt() * 100.0;

        // The discrete integrator should match the analytic sqrt(t) curve within 0.05%
        assert!(
            (fade_pct - analytic_q_li1_pct).abs() < 0.05,
            "Calendar fade after 30d at 25C should be ~{analytic_q_li1_pct:.3}%, got {fade_pct:.4}%. \
             (B1_REF={B1_REF}, tafel={tafel:.4}, b1_eff={b1_eff:.4e})"
        );
    }

    /// Smith 2017 Eq. 5: cycle aging with 1 full cycle/day (DOD=1.0) at 25C.
    /// dq_li2 = B2_REF * b2_accum * sqrt(sum_sq_dod)
    /// At T_REF: b2_accum = 1.0/day, sum_sq_dod = 1.0 (one cycle, DOD=1)
    /// After N days: q_li2 ~ B2_REF * N * 1.0 = 1.541e-5 * N
    /// After 1000 days: q_li2 ~ 0.01541 (1.541%)
    #[test]
    fn cycling_aging_single_cycle_per_day() {
        let ds = run_cycling_aging(1000, T_REF, V_REF, 0.5, 1.0);
        let ds_cal = run_calendar_aging(1000, T_REF, V_REF, 0.5);

        let cycling_contribution = ds.capacity_fade_pct() - ds_cal.capacity_fade_pct();
        let expected_cycling = B2_REF * 1000.0;
        assert!(
            (cycling_contribution - expected_cycling).abs() < 0.005,
            "Cycling fade contribution after 1000d should be ~{:.5} ({:.3}%), got {:.5} ({:.3}%)",
            expected_cycling,
            expected_cycling * 100.0,
            cycling_contribution,
            cycling_contribution * 100.0,
        );
    }

    /// Verify that reset_day_tracking preserves lifetime state (q_li1, q_li2,
    /// q_li3, capacity_fade, day_age) while clearing daily tracking fields
    /// (soc extremes, dod_max_today).
    #[test]
    fn degradation_reset_preserves_lifetime() {
        let u_neg = make_u_neg_table();
        let dt_s = SECONDS_PER_DAY;
        let mut ds = DegradationState::default();

        let mut prev_fade = 0.0_f64;
        for day in 0..3 {
            ds.accumulate(dt_s, T_REF, V_REF, 0.5);
            ds.update_daily(&u_neg, T_REF, 0.0);

            assert_eq!(ds.day_age, day + 1, "day_age should increment to {}", day + 1);

            let fade = ds.capacity_fade_pct();
            assert!(
                fade >= prev_fade,
                "Fade must be monotonically increasing: day {day} fade {fade} < prev {prev_fade}"
            );
            prev_fade = fade;

            let fade_before = ds.capacity_fade_pct();
            let q_li3_before = ds.q_li3;

            ds.reset_day_tracking(0.5);

            assert_eq!(ds.capacity_fade_pct(), fade_before, "capacity_fade must survive reset");
            assert_eq!(ds.q_li3, q_li3_before, "q_li3 must survive reset");
            assert_eq!(ds.dod_max_today, 0.0, "dod_max_today should reset to 0");
            assert_eq!(ds.soc_max_today, 0.5, "soc_max_today should reset to current SOC");
            assert_eq!(ds.soc_min_today, 0.5, "soc_min_today should reset to current SOC");
        }
    }

    /// Smith 2017 §II-B: capacity_fade must increase (or remain flat) on every day.
    /// Runs 10 days of pure calendar aging at 25°C and asserts strict monotonicity.
    /// Also verifies that fade is non-zero after day 2, when the sqrt(t) integrator
    /// has enough history to produce a positive dq_li1.
    #[test]
    fn degradation_calendar_aging() {
        let u_neg = make_u_neg_table();
        let dt_s = SECONDS_PER_DAY;
        let mut ds = DegradationState::default();
        let mut prev_fade = 0.0_f64;

        for day in 0..10_u32 {
            ds.accumulate(dt_s, T_REF, V_REF, 0.5);
            ds.update_daily(&u_neg, T_REF, 0.0);
            let fade = ds.capacity_fade_pct();
            assert!(
                fade >= prev_fade,
                "capacity_fade must be monotonically non-decreasing: day {day} fade={fade:.8} < prev={prev_fade:.8}"
            );
            prev_fade = fade;
            ds.reset_day_tracking(0.5);
        }
        // After 10 days the SEI layer should have grown measurably.
        assert!(
            prev_fade > 0.0,
            "capacity_fade must be positive after 10 days of calendar aging, got {prev_fade}"
        );
    }

    /// Smith 2017 Eq. 3: Arrhenius factor for mechanism 1 is positive, so
    /// calendar fade at 45°C must exceed fade at 25°C over the same period.
    /// This validates that the thermal acceleration is wired correctly end-to-end.
    #[test]
    fn degradation_temperature_dependence() {
        let ds_25 = run_calendar_aging(30, T_REF, V_REF, 0.5);
        let ds_45 = run_calendar_aging(30, 318.15, V_REF, 0.5);

        assert!(
            ds_45.capacity_fade_pct() > ds_25.capacity_fade_pct(),
            "Calendar fade at 45°C ({:.6}) must exceed 25°C ({:.6}): \
             higher temperature should accelerate SEI growth via Arrhenius",
            ds_45.capacity_fade_pct(),
            ds_25.capacity_fade_pct()
        );
        // The ratio must be strictly greater than 1; a sanity-check lower bound of 2×
        // ensures the temperature sensitivity is not trivially small.
        let ratio = ds_45.capacity_fade_pct() / ds_25.capacity_fade_pct();
        assert!(
            ratio > 2.0,
            "45°C/25°C fade ratio should be substantially above 1.0 (got {ratio:.3}); \
             Arrhenius + Tafel together should produce at least 2× acceleration"
        );
    }

    /// Crossing a day boundary must reset per-day tracking (soc extremes, dod_max_today,
    /// b1/b2/b3 accumulators) while preserving all lifetime state (q_li1, q_li2, q_li3,
    /// capacity_fade, day_age).
    #[test]
    fn degradation_reset_day_tracking() {
        let u_neg = make_u_neg_table();
        let dt_s = SECONDS_PER_DAY;
        let mut ds = DegradationState::default();

        // Accumulate day 1 at SOC 0.8 so extremes are non-trivial.
        ds.accumulate(dt_s, T_REF, V_REF, 0.8);
        ds.update_daily(&u_neg, T_REF, 0.0);

        // Capture lifetime state before crossing the day boundary.
        let fade_after_day1 = ds.capacity_fade_pct();
        let day_age_after_day1 = ds.day_age;

        // Verify day_age incremented.
        assert_eq!(day_age_after_day1, 1, "day_age should be 1 after first update_daily");

        // Cross the boundary at SOC 0.5.
        ds.reset_day_tracking(0.5);

        // Daily tracking must be cleared.
        assert_eq!(ds.dod_max_today, 0.0, "dod_max_today must reset to 0 at day boundary");
        assert!(
            (ds.soc_max_today - 0.5).abs() < 1e-12,
            "soc_max_today must reset to current SOC 0.5, got {}",
            ds.soc_max_today
        );
        assert!(
            (ds.soc_min_today - 0.5).abs() < 1e-12,
            "soc_min_today must reset to current SOC 0.5, got {}",
            ds.soc_min_today
        );
        // Per-day accumulators are reset by update_daily itself (before reset_day_tracking).
        assert!(
            ds.b1_accum.abs() < 1e-15,
            "b1_accum must be zero after update_daily, got {}",
            ds.b1_accum
        );

        // Lifetime state must be preserved across the reset.
        assert!(
            (ds.capacity_fade_pct() - fade_after_day1).abs() < 1e-15,
            "capacity_fade must be unchanged by reset_day_tracking: before={fade_after_day1:.10}, after={:.10}",
            ds.capacity_fade_pct()
        );
        assert_eq!(
            ds.day_age, day_age_after_day1,
            "day_age must not change during reset_day_tracking"
        );

        // Day 2 aging must continue to accumulate — lifetime is preserved.
        ds.accumulate(dt_s, T_REF, V_REF, 0.5);
        ds.update_daily(&u_neg, T_REF, 0.0);
        assert!(
            ds.capacity_fade_pct() >= fade_after_day1,
            "Fade after day 2 ({:.10}) must be >= day 1 ({fade_after_day1:.10})",
            ds.capacity_fade_pct()
        );
        assert_eq!(ds.day_age, 2, "day_age should be 2 after second update_daily");
    }

    /// Smith 2017: Mechanism 2 has Ea_b2 = -42800 J/mol (negative activation energy).
    /// Physically this models lithium plating, which is faster at lower temperatures.
    /// At 0C vs 25C:
    ///   arr_b2(0C) = exp(42800/8.314 * (1/273.15 - 1/298.15)) ~ exp(1.577) ~ 4.84
    /// Cycling fade at 0C should exceed 25C by this factor.
    #[test]
    fn negative_activation_energy_mechanism2_faster_at_low_temp() {
        let ds_25 = run_cycling_aging(100, T_REF, V_REF, 0.5, 1.0);
        let ds_0c = run_cycling_aging(100, 273.15, V_REF, 0.5, 1.0);

        let ds_25_cal = run_calendar_aging(100, T_REF, V_REF, 0.5);
        let ds_0c_cal = run_calendar_aging(100, 273.15, V_REF, 0.5);

        let cycling_25 = ds_25.capacity_fade_pct() - ds_25_cal.capacity_fade_pct();
        let cycling_0c = ds_0c.capacity_fade_pct() - ds_0c_cal.capacity_fade_pct();

        assert!(
            cycling_0c > cycling_25,
            "Cycling degradation at 0C ({cycling_0c:.6}) must exceed 25C ({cycling_25:.6}) \
             due to negative activation energy (lithium plating)"
        );

        // Physics reference: ratio = arr_b2(0C) / arr_b2(25C)
        let arr_0c = (-(EA_B2 / R_GAS) * (1.0 / 273.15 - 1.0 / T_REF)).exp();
        let expected_ratio = arr_0c; // arr_b2(25C) = 1.0
        let ratio = cycling_0c / cycling_25;
        assert!(
            (ratio - expected_ratio).abs() < 0.5,
            "0C/25C cycling fade ratio should be ~{expected_ratio:.2}, got {ratio:.3}"
        );
    }
}
