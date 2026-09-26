//! Battery degradation model (Smith et al. 2017, IEEE 7963578 / NREL
//! CP-5400-67102, "Life Prediction Model for Grid-Connected Li-ion Battery
//! Energy Storage System", 2017 American Control Conference) and rainflow
//! cycle counter.
//!
//! Provenance of the constants: every value was verified against BOTH the
//! NREL preprint of the paper and NREL's own reference implementation of
//! it — SSC `shared/lib_battery_lifetime_nmc.{h,cpp}`, the model EnergyPlus
//! reaches through its SSC delegation. The PDF's inner-fraction minus
//! glyphs are unmapped in every text extractor, so the paper alone cannot
//! settle the sign conventions; the reference implementation is decisive
//! and it confirms α_b1 = −1 (the physically-correct direction: a
//! more-lithiated anode ages faster) while fixing b3_ref and θ as positive
//! and b0 = 1.07 — the constants below match it exactly.

use hares_types::HaresError;
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

    /// Weighted DOD-power sum accumulated today: Σ(count_i × DOD_i^beta).
    /// Used by the degradation model's cycle terms — beta = 2 for the Li
    /// branch's Miner-rule damage (Smith 2017 Eq. 4's b2·N weighting),
    /// beta = βc2 = 4.54 for the negative-electrode site-loss branch
    /// (Eq. 11).
    pub(crate) fn sum_dod_pow_daily(&self, beta: f64) -> f64 {
        self.daily_cycle_dods
            .iter()
            .map(|&(range, count)| {
                // Exact multiply for the integer-power case (bit-identical
                // to the pre-generalization Σ count·range² the Li branch
                // has always used); powf for the fractional βc2 = 4.54.
                let dod_pow = if beta == 2.0 {
                    range * range
                } else {
                    range.powf(beta)
                };
                count * dod_pow
            })
            .sum()
    }

    /// Σ(count_i × DOD_i²) — the Li branch's cycle-damage sum (`beta = 2`
    /// specialization of [`Self::sum_dod_pow_daily`]).
    pub(crate) fn sum_squared_dod_daily(&self) -> f64 {
        self.sum_dod_pow_daily(2.0)
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

/// Model coefficients — Smith 2017 (NREL/CP-5400-67102, Eq. 4–11 and the
/// fitted-parameter list), verified against NREL's reference implementation
/// of the paper (SSC `lib_battery_lifetime_nmc.{h,cpp}`).
mod deg_const {
    // Reference constants — Smith 2017 §II: "common reference constants
    // Tref = 298.15 K, Vref = 3.7 V, and U-,ref = 0.08 V".
    pub const R_GAS: f64 = 8.314; // J/(K·mol)
    pub const F_FARADAY: f64 = 96_485.0; // A·s/mol
    pub const T_REF: f64 = 298.15; // K (25 °C)
    pub const V_REF: f64 = 3.7; // V reference OCV
    pub const U_NEG_REF: f64 = 0.08; // V reference negative electrode potential

    // Li branch, mechanism 1 — calendar SEI growth (sqrt-of-time, Eq. 5).
    // Fitted list: b1,ref = 3.503e-3 day^-0.5, Ea,b1 = 35392 J/mol,
    // γ = 2.472, βb1 = 2.157. αb1 = −1 per SSC (`alpha_a_b1 = -1`); also
    // the physically-correct direction: a more-lithiated anode (lower
    // U_neg) ages faster, which exp(−αF/R·(U/T − Uref/Tref)) gives only
    // for α < 0.
    pub const B1_REF: f64 = 3.503e-3; // day^-0.5
    pub const EA_B1: f64 = 35_392.0; // J/mol
    pub const ALPHA_B1: f64 = -1.0; // Tafel symmetry factor
    pub const BETA_B1: f64 = 2.157; // DOD power-law exponent
    pub const GAMMA_B1: f64 = 2.472; // DOD coupling coefficient

    // Li branch, mechanism 2 — cycle aging (Eq. 4's −b2·N, b2 from Eq. 6).
    // Fitted list: b2,ref = 1.541e-5, Ea,b2 = −42800 J/mol
    // (negative ⟹ faster at lower T).
    pub const B2_REF: f64 = 1.541e-5; // cycle^-1
    pub const EA_B2: f64 = -42_800.0; // J/mol (negative ⟹ faster at lower T)

    // Li branch, mechanism 3 — break-in Li loss at BOL (Eq. 7): a small Li
    // loss growing over the first ~τ days and deepening with DOD — Eq. 4's
    // −b3(1−exp(−t/τ)). Fitted list: b3,ref = 2.805e-2, Ea,b3 = 42800
    // J/mol, αb3 = 0.0066, τb3 = 5 days, θ = 0.135 (both positive per SSC:
    // `b3_ref = 0.02805`, `theta = 0.135`).
    pub const B3_REF: f64 = 2.805e-2; // dimensionless
    pub const EA_B3: f64 = 42_800.0; // J/mol
    pub const ALPHA_B3: f64 = 0.0066; // Tafel factor for V_oc
    pub const TAU_B3: f64 = 5.0; // days
    pub const THETA: f64 = 0.135; // DOD coupling

    // Li branch, BOL intercept — fitted list: b0 = 1.07 (the sqrt-time
    // fit's extrapolated t=0 intercept), scaled by d0,ref/Ah,ref =
    // 75.075 Ah / 75 Ah (SSC: d0_ref, Ah_ref — the reference cell's
    // measured-over-nameplate ratio). Eq. 3's temperature dependence of d0
    // (Ea,d0,1 = 4126, Ea,d0,2 = 9.752e6 J/mol) is NOT applied here: the
    // stationary Battery already carries that exact Arrhenius as its
    // `CapacityDerateModel` (battery/mod.rs), so applying it in both layers
    // would double-count; this model uses the T-reference ratio and each
    // equipment applies its own reversible temperature derate.
    pub const B0: f64 = 1.07;
    /// d0,ref / Ah,ref — the reference cell's measured-over-nameplate
    /// capacity ratio (SSC: 75.075 Ah / 75 Ah).
    pub const D0_REL: f64 = 75.075 / 75.0;

    // Negative-electrode site-loss branch (Eq. 8–11): active sites lost per
    // cycle, inversely proportional to the remaining sites (the graphite
    // anode's ~8 % volume change per full discharge stresses remaining
    // sites more as they are lost). SSC fitted values: c0,ref = 75.675 Ah,
    // c2,ref = 5.226e-5 Ah/cycle (Ea,c2 = −48260 J/mol), βc2 = 4.54 —
    // taken from **current SSC trunk**
    // (github.com/NREL/ssc@develop,
    // `lib_battery_lifetime_nmc.h`), which carries the rainflow cycle
    // model and these fitted values. The SSC copy vendored under
    // `vendors/EnergyPlus/third_party/ssc` is an OLDER revision (c0,ref
    // = 75.64, c2,ref = 0.0039193, pre-rainflow daily formulation) —
    // verifying against the vendored copy will show a mismatch; trunk
    // is the lineage the ticket names as decisive.
    // The branch's capacity LEVEL is evaluated at its T-reference ratio
    // (c0,ref/Ah,ref): SSC scales the level with c0's own Arrhenius
    // (Ea,c0 = 2224 J/mol), but HARES factors all reversible
    // temperature-capacity scaling out of the degradation fade — the
    // stationary Battery already carries the NREL d0 Arrhenius (Ea,d0,1 =
    // 4126, Ea,d0,2 = 9.752e6 J/mol) as its `CapacityDerateModel`, and
    // applying a second scaling inside the fade would double-count. The
    // divergence is bounded and reversible: at 15 °C SSC's level scale is
    // arr_c0 = 0.963 vs the d0 Arrhenius 0.943 — a ≤2 % difference in the
    // BOL regime where the negative-electrode branch binds. The DAMAGE
    // path (c2's Arrhenius, βc2 weighting) is unaffected — it is
    // temperature-scaled exactly as SSC scales it.
    pub const C0_REF_AH: f64 = 75.675; // Ah — initial negative-site capacity
    pub const C2_REF_AH_PER_CYCLE: f64 = 5.226e-5; // Ah/cycle
    pub const EA_C2: f64 = -48_260.0; // J/mol (negative ⟹ faster at lower T)
    pub const BETA_C2: f64 = 4.54; // DOD power-law exponent
    /// Nameplate capacity of the Smith 2017 reference cell (SSC: Ah_ref).
    pub const AH_REF: f64 = 75.0;
}

/// Full Smith 2017 (IEEE 7963578 / NREL CP-5400-67102) battery degradation
/// model: the Li-limiting branch (three loss mechanisms, Eq. 4–7) and the
/// negative-electrode site-loss branch (Eq. 8–11), with the usable
/// capacity fraction `min(QLi, Qneg)` per Eq. 1.
///
/// Li-limiting mechanisms (per-unit losses accumulated against the BOL
/// intercept):
///   `q_li1` -- calendar SEI growth (sqrt-of-time, Arrhenius + Tafel + DOD)
///   `q_li2` -- cycle-induced lithium loss (Arrhenius + rainflow DOD damage)
///   `q_li3` -- break-in Li loss at BOL (exponential relaxation, Eq. 4's
///              −b3(1−exp(−t/τ)): a small LOSS growing over the first ~τ
///              days, deepening with DOD)
///
/// Negative-electrode branch: site capacity lost per cycle inversely
/// proportional to remaining sites (`dq_neg_ah`, Ah — the graphite anode's
/// volume-change fatigue), with the same Arrhenius-in-time accumulation
/// pattern the Li branch uses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DegradationState {
    // ---- Per-timestep accumulators (reset each day) ----
    /// Σ b1_ref · exp(−Ea_b1/R · (1/T − 1/T_ref)) · dt_day
    pub(crate) b1_accum: f64,
    /// Σ exp(−Ea_b2/R · (1/T − 1/T_ref)) · dt_day
    b2_accum: f64,
    /// Σ b3_ref · arr_b3 · tafel_b3 · (1 + θ·DOD_max) · dt_day
    b3_accum: f64,
    /// Σ c2_ref · exp(−Ea_c2/R · (1/T − 1/T_ref)) · dt_day — the site-loss
    /// damage-rate integral (the branch's capacity level is evaluated at
    /// its T-reference ratio; see the `deg_const` notes).
    c2_accum: f64,

    // ---- Daily temperature tracking (reset each day) ----
    // Smith 2017 Eq.4: the Tafel correction for mechanism 1 (tafel_b1) is applied
    // once per day to the accumulated b1 term.  It must use a representative
    // daily temperature, not the temperature at the first timestep of the
    // following day (which was the pre-fix bug).  We accumulate the running sum
    // of cell temperature across all timesteps of the day and compute the mean
    // at the midnight boundary in `update_daily()`.
    sum_cell_temp_k: f64,
    n_temp_samples: u64,

    // ---- Cumulative lithium losses ----
    pub(crate) q_li1: f64,
    pub(crate) q_li2: f64,
    /// Break-in Li loss at BOL (Eq. 4's b3 term, a positive loss relaxing
    /// toward the running b3 integral over ~τ days).
    pub(crate) q_li3: f64,

    // ---- Negative-electrode site-loss branch (Eq. 8–11) ----
    /// Cumulative negative-electrode site loss [Ah].
    pub(crate) dq_neg_ah: f64,

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
    /// Computed capacity fade fraction — 1 − min(QLi, Qneg) per Eq. 1,
    /// floored at a 0 usable-capacity lower bound. Negative values mean
    /// the modeled capacity is (transiently) above the nameplate rating —
    /// the reference model's BOL state (b0 = 1.07 intercept, capped by the
    /// negative-electrode branch at ≈ +0.9 %).
    pub(crate) capacity_fade: f64,
}

impl Default for DegradationState {
    fn default() -> Self {
        Self {
            b1_accum: 0.0,
            b2_accum: 0.0,
            b3_accum: 0.0,
            c2_accum: 0.0,
            sum_cell_temp_k: 0.0,
            n_temp_samples: 0,
            q_li1: 0.0,
            q_li2: 0.0,
            q_li3: 0.0,
            dq_neg_ah: 0.0,
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
    pub(crate) fn capacity_fade_fraction(&self) -> f64 {
        self.capacity_fade
    }

    /// Cycle-aging Arrhenius accumulator (Σ exp(−Ea_b2/R · (1/T − 1/T_ref)) · dt_day).
    /// Exposed for invariant checks after the midnight reset.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    pub(crate) fn b2_accum(&self) -> f64 {
        self.b2_accum
    }

    /// BOL-transient accumulator (Σ b3_ref · arr · tafel · (1+θ·DOD) · dt_day).
    /// Exposed for invariant checks after the midnight reset.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    pub(crate) fn b3_accum(&self) -> f64 {
        self.b3_accum
    }

    /// Called every simulation timestep to accumulate sub-daily degradation terms.
    ///
    /// `dt_s`        -- timestep in seconds
    /// `cell_temp_k` -- cell temperature in kelvin
    /// `v_oc`        -- cell open-circuit voltage at current SOC (V)
    /// `soc`         -- current state of charge [0..1]
    ///
    /// # Cell-temperature domain guard
    ///
    /// Two layers, stated separately:
    ///
    /// 1. **Error bounds** — [−100 °C, +130 °C]. Outside this envelope no
    ///    simulated Li-ion pack can exist (below −100 °C the electrolyte has
    ///    frozen solid; above ~130 °C the cell has entered thermal runaway
    ///    and ceased to exist as a battery), so a temperature outside it is
    ///    a broken thermal model upstream, and it errors loudly rather than
    ///    being fed to the fit. An EV model that attributes charger
    ///    conversion losses to the pack produces 165–281 °C pack temperatures
    ///    — this guard is the defense-in-depth that makes that class fail the
    ///    simulation instead of silently returning negative capacity fade.
    /// 2. **Validated domain** — Smith 2017's aging tests span 0 °C to 55 °C
    ///    (NREL/CP-5400-67102 Table I: 0, 23, 30, 45, 55 °C conditions).
    ///    HARES deliberately operates below 0 °C in cold climates — erroring
    ///    there would refuse to simulate the cold-climate regime residential
    ///    load simulation must cover. The extrapolation is owned, not silent:
    ///    with the negative `EA_B2` the cycle term runs ≈8× the 25 °C rate at
    ///    −7 °C (≈32× at −25 °C), while the calendar and BOL terms slow down;
    ///    preconditioning moves plugged-in packs into the validated domain
    ///    for the charging hours that dominate the aging arithmetic.
    pub(crate) fn accumulate(
        &mut self,
        dt_s: f64,
        cell_temp_k: f64,
        v_oc: f64,
        soc: f64,
    ) -> Result<(), HaresError> {
        use deg_const::*;

        const CELL_TEMP_ERROR_MIN_K: f64 = 173.15; // −100 °C: frozen electrolyte
        const CELL_TEMP_ERROR_MAX_K: f64 = 403.15; // +130 °C: thermal runaway
        if !(cell_temp_k.is_finite()
            && (CELL_TEMP_ERROR_MIN_K..=CELL_TEMP_ERROR_MAX_K).contains(&cell_temp_k))
        {
            return Err(HaresError::Equipment(format!(
                "battery degradation called with cell temperature {} K ({} °C) \
                 outside the physically implausible envelope [-100, +130] °C; \
                 the upstream thermal model is broken (e.g. conversion losses \
                 attributed to the pack as heat)",
                cell_temp_k,
                cell_temp_k - 273.15
            )));
        }

        let dt_day = dt_s / SECONDS_PER_DAY;
        let t = cell_temp_k;
        let inv_diff = 1.0 / t - 1.0 / T_REF;

        // Track running temperature for the daily mean used by the Tafel
        // correction in update_daily().  Each step's contribution to b1_accum
        // is already weighted by that step's Arrhenius factor, but the Tafel
        // correction (tafel_b1) is a single daily multiplier — the mean cell
        // temperature is the physically appropriate representative value.
        self.sum_cell_temp_k += cell_temp_k;
        self.n_temp_samples += 1;

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

        // Mechanism 3: BOL break-in loss -- use running dod_max_today as best estimate
        let tafel_b3 = ((ALPHA_B3 * F_FARADAY / R_GAS) * (v_oc / t - V_REF / T_REF)).exp();
        self.b3_accum +=
            B3_REF * (-(EA_B3 / R_GAS) * inv_diff).exp() * tafel_b3 * (1.0 + THETA * dod) * dt_day;

        // Negative-electrode branch (Eq. 8-11): the site-loss damage-rate
        // integral (c2 carries its reference coefficient, exactly as b1
        // does). The branch's capacity level carries no temperature
        // integral — it is evaluated at the T-reference ratio in
        // `update_daily` (see the `deg_const` notes on the factored
        // reversible-temperature architecture).
        self.c2_accum += C2_REF_AH_PER_CYCLE * (-(EA_C2 / R_GAS) * inv_diff).exp() * dt_day;
        Ok(())
    }

    /// Reset daily tracking fields at the start of each new day.
    /// Called after `update_daily()` to prepare for the next 24-hour window.
    pub(crate) fn reset_day_tracking(&mut self, soc: f64) {
        self.soc_max_today = soc;
        self.soc_min_today = soc;
        self.dod_max_today = 0.0;
        self.soc_at_max_dod = soc;
    }

    /// Representative daily cell temperature: the arithmetic mean of all
    /// per-step cell temperatures accumulated during the day.
    ///
    /// Falls back to `T_REF` (25 °C) when no samples have been recorded —
    /// this only occurs if `update_daily()` is called before any
    /// `accumulate()` call, which does not happen in normal operation.
    pub(crate) fn daily_mean_temp_k(&self) -> f64 {
        if self.n_temp_samples > 0 {
            self.sum_cell_temp_k / self.n_temp_samples as f64
        } else {
            deg_const::T_REF
        }
    }

    /// Called once per day (at midnight) to compute lithium-loss increments and
    /// update the cumulative capacity fade.
    ///
    /// `u_neg_table`        -- negative electrode potential lookup table
    /// `sum_squared_dod`    -- Σ(count_i × DOD_i²) from today's rainflow cycles,
    ///                        where count_i is 0.5 for half-cycles and 1.0 for full cycles.
    ///
    /// The representative daily cell temperature for the Tafel correction is
    /// computed internally from the running mean accumulated by `accumulate()`,
    /// eliminating the previous bug where the caller could pass the
    /// first-of-new-day temperature.
    /// Called once per day (at midnight) to compute the day's loss
    /// increments and update the cumulative capacity fade — the Li branch
    /// (Eq. 4–7) and the negative-electrode branch (Eq. 8–11), usable
    /// capacity = min(QLi, Qneg) per Eq. 1.
    ///
    /// `u_neg_table` -- negative electrode potential lookup table
    /// `rainflow`    -- the cycle counter; the DOD-damage sums are computed
    ///                 here so callers cannot mix up the two betas
    ///                 (2 for the Li branch, βc2 for the site-loss branch).
    ///
    /// The representative daily cell temperature for the Tafel correction is
    /// computed internally from the running mean accumulated by `accumulate()`,
    /// eliminating the previous bug where the caller could pass the
    /// first-of-new-day temperature.
    pub(crate) fn update_daily(&mut self, u_neg_table: &UNegTable, rainflow: &RainflowCounter) {
        use deg_const::*;

        let t_day = self.daily_mean_temp_k();
        let dod_max = self.dod_max_today;

        // ---- Li branch, mechanism 1: Tafel and DOD corrections ----
        let u_neg = u_neg_table.potential_at_soc(self.soc_at_max_dod);
        let tafel_b1 = ((ALPHA_B1 * F_FARADAY / R_GAS) * (u_neg / t_day - U_NEG_REF / T_REF)).exp();
        let b1_eff = self.b1_accum * tafel_b1 * (GAMMA_B1 * dod_max.powf(BETA_B1)).exp();

        // ---- Li branch: the day's loss increments ----

        // Mechanism 1: calendar SEI (sqrt-of-time progression)
        let dq_li1 = if self.q_li1.abs() < 1e-5 && self.day_age > 0 {
            b1_eff / (self.day_age as f64).sqrt()
        } else if self.q_li1.abs() >= 1e-5 {
            0.5 * b1_eff.powi(2) / self.q_li1
        } else {
            // day_age == 0: first day, skip to avoid division-by-zero
            0.0
        };

        // ── Mechanism 2: cycle aging -- Smith 2017 Eq. 4's −b2·N, b2 from
        // Eq. 6, with the cycle count DOD-weighted per Miner's rule ──
        //
        // dq_li2 = b2_ref × Σ_t[arr(T_t) × dt_t] × √Σ_i[count_i × DOD_i²]
        //
        // Deviation from OCHRE (vendors/OCHRE/ochre/Equipment/Battery.py:418):
        //   OCHRE: b2 = b2_ref × √Σ_i[(arr_i × count_i)²] / deg_time
        //     • Computes Arrhenius per-cycle using each cycle's avg temperature
        //     • Omits DOD amplitude entirely -- a 10% DOD cycle and a 100% DOD
        //       cycle contribute identical fade, violating Miner's rule.
        //   HARES: accumulates Arrhenius every timestep (finer time resolution),
        //     then weights cycle damage by Σ(count × DOD²) per Miner's linear
        //     damage rule (fatigue damage ∝ stress amplitude squared) — the
        //     standard electrochemical cycle-life aggregation (Schmalstieg
        //     2014, Xu 2018). NREL's SSC aggregates this term differently
        //     again (b2_ref·b2²·√Σ(DOD·count)²); the three forms differ only
        //     in the damage aggregation, and HARES's is the documented
        //     deliberate choice.
        let dq_li2 = B2_REF * self.b2_accum * rainflow.sum_squared_dod_daily().sqrt();

        // Mechanism 3: break-in Li loss -- exponential relaxation of the
        // loss toward the running b3 integral (Eq. 4's −b3(1−exp(−t/τ))).
        // The step is non-negative: a loss that accumulates over the first
        // ~τ days and stops once it reaches the integral — never a reversal.
        let dq_li3 = (self.b3_accum - self.q_li3).max(0.0) / TAU_B3;

        self.q_li1 += dq_li1;
        self.q_li2 += dq_li2;
        self.q_li3 += dq_li3;

        // Li-branch usable capacity: d0,ref/Ah,ref × (b0 − Σ losses).
        let q_li_rel = D0_REL * (B0 - self.q_li1 - self.q_li2 - self.q_li3);

        // ---- Negative-electrode branch (Eq. 8–11) ----
        // Site capacity lost per cycle, inversely proportional to the
        // remaining sites (the runaway term c0/(c0 − dq): as sites are lost,
        // the survivors are stressed more). DOD-weighted with βc2.
        let c2_ah = self.c2_accum * rainflow.sum_dod_pow_daily(BETA_C2).sqrt();
        let dq_neg = if self.dq_neg_ah < C0_REF_AH {
            c2_ah * C0_REF_AH / (C0_REF_AH - self.dq_neg_ah)
        } else {
            0.0
        };
        self.dq_neg_ah += dq_neg;
        // The branch's capacity level at its T-reference ratio (SSC's
        // c0,ref/Ah,ref — see the `deg_const` notes for why the
        // temperature scaling is factored out to the equipment layer),
        // times the remaining-site fraction.
        let q_neg_rel = (C0_REF_AH / AH_REF) * (1.0 - self.dq_neg_ah);

        // ---- Usable capacity: min(QLi, Qneg) per Eq. 1, floored at 0 ----
        let q_relative = q_li_rel.min(q_neg_rel).max(0.0);
        self.capacity_fade = 1.0 - q_relative;

        // ---- Reset accumulators for next day ----
        self.b1_accum = 0.0;
        self.b2_accum = 0.0;
        self.b3_accum = 0.0;
        self.c2_accum = 0.0;
        self.sum_cell_temp_k = 0.0;
        self.n_temp_samples = 0;
        self.day_age += 1;
        // soc extremes and dod_max are reset by the caller via reset_day_tracking().
    }
}

#[cfg(test)]
mod tests {
    use super::deg_const::*;
    use super::*;

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
    ///     sum_squared_dod = 0.5 × 0.8² = 0.32  (Smith 2017 b2 input term)
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

    /// Midnight boundary: a half-cycle started before midnight must survive
    /// `reset_daily()` and complete correctly after midnight, with the
    /// extracted cycle attributed to the *new* day's `sum_squared_dod_daily()`.
    ///
    /// Sequence: pre-midnight [0.2, 0.8] (charging ramp, no reversal —
    /// buffer has 2 points, no extraction).  After reset_daily(), push 0.2
    /// which reverses direction, completing a half-cycle with DOD=0.6.
    ///
    /// This verifies the Finding 2 fix: the reversal buffer is preserved
    /// across reset_daily(), and the completed cycle's DOD contributes to
    /// the new day's sum_squared_dod_daily(), not the old day's.
    #[test]
    fn rainflow_half_cycle_straddles_midnight_correctly() {
        let mut rc = RainflowCounter::default();

        // Pre-midnight: start a charging ramp.  Monotonic, so reversals = [0.2, 0.8].
        rc.push(0.2);
        rc.push(0.8);

        // No cycles extracted yet — need 3 points for extraction.
        let pre_midnight_cycles = rc.total_cycles();
        assert_eq!(
            pre_midnight_cycles, 0.0,
            "no cycles should be extracted from a 2-point ramp"
        );

        // Midnight: reset daily DOD list but preserve the reversal buffer.
        rc.reset_daily();
        assert_eq!(
            rc.reversals.len(),
            2,
            "reversal buffer must survive reset_daily"
        );
        assert_eq!(
            rc.sum_squared_dod_daily(),
            0.0,
            "sum_squared_dod_daily must be zero after reset_daily"
        );

        // Post-midnight: reverse direction, completing the half-cycle.
        // [0.2, 0.8, 0.2]: X=|0.2-0.8|=0.6, Y=|0.8-0.2|=0.6. X >= Y, n=3
        // → half-cycle extracted with range 0.6, count 0.5.
        rc.push(0.2);

        let post_midnight_cycles = rc.total_cycles();
        assert!(
            post_midnight_cycles > pre_midnight_cycles,
            "completing the straddling cycle must increment total_cycles: before={}, after={}",
            pre_midnight_cycles,
            post_midnight_cycles
        );
        // The extracted cycle's DOD must be attributed to the new day.
        let expected_sum_sq = 0.5 * 0.6_f64 * 0.6_f64;
        assert!(
            (rc.sum_squared_dod_daily() - expected_sum_sq).abs() < 1e-12,
            "sum_squared_dod_daily must be {expected_sum_sq:.4} (0.5×0.6²) after the straddling cycle, got {}",
            rc.sum_squared_dod_daily()
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
        let rf = RainflowCounter::default();
        for _ in 0..days {
            ds.accumulate(dt_s, cell_temp_k, v_oc, soc).unwrap();
            ds.update_daily(&u_neg, &rf);
            ds.reset_day_tracking(soc);
        }
        ds
    }

    /// Helper: run N days with a fixed DOD-damage sum per day: one
    /// half-cycle of range (2·sum_squared_dod)^½, whose 0.5·range² damage
    /// equals `sum_squared_dod_per_day`. The same counter is presented
    /// every day (its daily pairs are not reset), so both cycle branches
    /// (Li β=2 and negative-electrode βc2) see consistent daily damage.
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
        let mut rf = RainflowCounter::default();
        let r = (2.0 * sum_squared_dod_per_day).sqrt();
        rf.push(0.2);
        rf.push(0.2 + r);
        rf.push(0.2); // completes the reversal → half-cycle of range r
        for _ in 0..days {
            ds.accumulate(dt_s, cell_temp_k, v_oc, soc).unwrap();
            ds.update_daily(&u_neg, &rf);
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
        ds.accumulate(dt_s, T_REF, V_REF, 0.5).unwrap();

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
    /// q_li1 ratio between 45C and 25C should be ~3.40.
    ///
    /// Derivation: ratio = (arr_45 * tafel_45) / (arr_25 * tafel_25)
    ///   arr_25 = 1.0, tafel_25 = exp(-F/R * (u_neg/298.15 - 0.08/298.15)) ~ 0.1256
    ///   arr_45 = 2.454, tafel_45 = exp(-F/R * (u_neg/318.15 - 0.08/298.15)) ~ 0.1740
    ///   ratio = (2.454 * 0.1740) / (1.0 * 0.1256) ~ 3.40
    ///
    /// Uses q_li1 directly (not capacity_fade_fraction) since the BOL q_li3 term has a
    /// different temperature dependence that would skew the ratio.
    #[test]
    fn arrhenius_factor_at_45c_mechanism1() {
        let ds_25 = run_calendar_aging(30, T_REF, V_REF, 0.5);
        let ds_45 = run_calendar_aging(30, 318.15, V_REF, 0.5);

        // Compute physics reference ratio from Arrhenius + Tafel (mechanism 1 only)
        let arr_25 = 1.0_f64;
        let arr_45 = (-(EA_B1 / R_GAS) * (1.0 / 318.15 - 1.0 / T_REF)).exp();
        let tafel_25 = tafel_b1_factor(0.5, T_REF);
        let tafel_45 = tafel_b1_factor(0.5, 318.15);
        let expected_ratio = (arr_45 * tafel_45) / (arr_25 * tafel_25);

        // Compare q_li1 (mechanism 1) directly to isolate the Arrhenius effect.
        let ratio = ds_45.q_li1 / ds_25.q_li1;
        assert!(
            (ratio - expected_ratio).abs() < 0.1,
            "45C/25C q_li1 ratio should be ~{expected_ratio:.2}, got {ratio:.4} \
             (q_li1_25={:.6}, q_li1_45={:.6})",
            ds_25.q_li1,
            ds_45.q_li1
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
    ///
    /// Note: capacity_fade also includes q_li3 (BOL transient, negative at these conditions),
    /// so this test checks q_li1 directly rather than capacity_fade_fraction().
    #[test]
    fn calendar_aging_30_days_25c() {
        let ds = run_calendar_aging(30, T_REF, V_REF, 0.5);
        let q_li1_pct = ds.q_li1 * 100.0;

        // Compute physics reference: B1_REF * tafel * sqrt(30)
        let tafel = tafel_b1_factor(0.5, T_REF);
        let b1_eff = B1_REF * tafel; // arr = 1.0 at T_REF, dod_corr = 1.0 at dod=0
        let analytic_q_li1_pct = b1_eff * (30.0_f64).sqrt() * 100.0;

        // The discrete integrator should match the analytic sqrt(t) curve within 0.05%.
        // Check q_li1 directly -- capacity_fade also includes the q_li3 BOL term.
        assert!(
            (q_li1_pct - analytic_q_li1_pct).abs() < 0.05,
            "q_li1 after 30d at 25C should be ~{analytic_q_li1_pct:.3}%, got {q_li1_pct:.4}%. \
             (B1_REF={B1_REF}, tafel={tafel:.4}, b1_eff={b1_eff:.4e})"
        );
    }

    /// Smith 2017 Eq. 4–6 and 8–11: cycle aging with 1 full cycle/day
    /// (DOD=1.0) at 25C. The Li branch's cycle loss is
    /// dq_li2 = B2_REF·b2_accum·√Σ(count·DOD²) → after 1000 days ≈ 1.541 %;
    /// the negative-electrode branch's site loss accumulates in Ah on top
    /// (deep daily cycling is exactly the regime that branch exists for —
    /// the graphite anode's fatigue). Both are asserted on the mechanism
    /// state directly: through `fade` the min(QLi, Qneg) structure mixes
    /// the branches.
    #[test]
    fn cycling_aging_single_cycle_per_day() {
        let ds = run_cycling_aging(1000, T_REF, V_REF, 0.5, 1.0);
        let ds_cal = run_calendar_aging(1000, T_REF, V_REF, 0.5);

        // Li-branch cycle loss: the Miner-rule arithmetic, exact.
        let expected_q_li2 = B2_REF * 1000.0;
        assert!(
            (ds.q_li2 - expected_q_li2).abs() < 1e-9,
            "q_li2 after 1000 days of 1-cycle/day should be {expected_q_li2:.6}, got {:.6}",
            ds.q_li2
        );
        assert_eq!(ds_cal.q_li2, 0.0, "no cycling, no Li cycle loss");

        // Negative-electrode site loss: present only in the cycling run,
        // accumulating in Ah (the runaway c0/(c0−dq) factor keeps the step
        // essentially at c2·√Σ(count·DOD^βc2) per day here).
        assert_eq!(ds_cal.dq_neg_ah, 0.0);
        let r = 2.0_f64.sqrt(); // the helper's half-cycle range for sum_sq = 1.0
        let c2_damage_per_day = 0.5 * r.powf(BETA_C2);
        let expected_dq_neg = 1000.0 * C2_REF_AH_PER_CYCLE * c2_damage_per_day.sqrt();
        assert!(
            (ds.dq_neg_ah - expected_dq_neg).abs() < 0.05 * expected_dq_neg,
            "dq_neg after 1000 days should be ~{expected_dq_neg:.6} Ah, got {:.6}",
            ds.dq_neg_ah
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

        let mut prev_q_li1 = 0.0_f64;
        for day in 0..3 {
            ds.accumulate(dt_s, T_REF, V_REF, 0.5).unwrap();
            ds.update_daily(&u_neg, &RainflowCounter::default());

            assert_eq!(
                ds.day_age,
                day + 1,
                "day_age should increment to {}",
                day + 1
            );

            // During BOL transient, capacity_fade is negative (q_li3 dominates).
            // Assert q_li1 (calendar loss) is monotonically increasing instead.
            let q_li1 = ds.q_li1;
            assert!(
                q_li1 >= prev_q_li1,
                "q_li1 must be monotonically increasing: day {day} q_li1={q_li1} < prev={prev_q_li1}"
            );
            prev_q_li1 = q_li1;

            let fade_before = ds.capacity_fade_fraction();
            let q_li3_before = ds.q_li3;

            ds.reset_day_tracking(0.5);

            assert_eq!(
                ds.capacity_fade_fraction(),
                fade_before,
                "capacity_fade must survive reset"
            );
            assert_eq!(ds.q_li3, q_li3_before, "q_li3 must survive reset");
            assert_eq!(ds.dod_max_today, 0.0, "dod_max_today should reset to 0");
            assert_eq!(
                ds.soc_max_today, 0.5,
                "soc_max_today should reset to current SOC"
            );
            assert_eq!(
                ds.soc_min_today, 0.5,
                "soc_min_today should reset to current SOC"
            );
        }
    }

    /// Smith 2017 §II-B: q_li1 (calendar SEI growth) must increase monotonically.
    /// Runs 10 days of pure calendar aging at 25°C.
    /// capacity_fade is negative during the BOL transient (q_li3 dominates),
    /// so we assert on q_li1 directly to isolate mechanism 1 behaviour.
    #[test]
    fn degradation_calendar_aging() {
        let u_neg = make_u_neg_table();
        let dt_s = SECONDS_PER_DAY;
        let mut ds = DegradationState::default();
        let mut prev_q_li1 = 0.0_f64;

        for day in 0..10_u32 {
            ds.accumulate(dt_s, T_REF, V_REF, 0.5).unwrap();
            ds.update_daily(&u_neg, &RainflowCounter::default());
            let q_li1 = ds.q_li1;
            assert!(
                q_li1 >= prev_q_li1,
                "q_li1 must be monotonically non-decreasing: day {day} q_li1={q_li1:.8} < prev={prev_q_li1:.8}"
            );
            prev_q_li1 = q_li1;
            ds.reset_day_tracking(0.5);
        }
        // After 10 days the SEI layer should have grown measurably.
        assert!(
            prev_q_li1 > 0.0,
            "q_li1 must be positive after 10 days of calendar aging, got {prev_q_li1}"
        );
        // The break-in loss is positive and converging (a LOSS, per the
        // reference model — not the pre-fix negative "boost").
        assert!(
            ds.q_li3 > 0.0,
            "q_li3 must be a positive break-in loss, got {}",
            ds.q_li3
        );
        // And the BOL usable capacity sits above nameplate (negative fade,
        // the min(QLi, Qneg) state: Li ≈ +7 %, capped by the negative-
        // electrode branch at ≈ +0.9 %).
        assert!(
            ds.capacity_fade < 0.0,
            "capacity fade at BOL must be negative (capacity above nameplate), got {}",
            ds.capacity_fade
        );
    }

    /// Smith 2017 Eq. 3: Arrhenius factor for mechanism 1 is positive, so q_li1
    /// (calendar SEI growth) at 45°C must exceed that at 25°C over the same period.
    /// This validates that the thermal acceleration is wired correctly end-to-end.
    /// Uses q_li1 directly -- capacity_fade also includes q_li3 which has its own
    /// temperature dependence and is negative during the BOL transient.
    #[test]
    fn degradation_temperature_dependence() {
        let ds_25 = run_calendar_aging(30, T_REF, V_REF, 0.5);
        let ds_45 = run_calendar_aging(30, 318.15, V_REF, 0.5);

        assert!(
            ds_45.q_li1 > ds_25.q_li1,
            "q_li1 at 45°C ({:.6}) must exceed 25°C ({:.6}): \
             higher temperature should accelerate SEI growth via Arrhenius",
            ds_45.q_li1,
            ds_25.q_li1
        );
        // The ratio must be strictly greater than 1; a sanity-check lower bound of 2×
        // ensures the temperature sensitivity is not trivially small.
        let ratio = ds_45.q_li1 / ds_25.q_li1;
        assert!(
            ratio > 2.0,
            "45°C/25°C q_li1 ratio should be substantially above 1.0 (got {ratio:.3}); \
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
        ds.accumulate(dt_s, T_REF, V_REF, 0.8).unwrap();
        ds.update_daily(&u_neg, &RainflowCounter::default());

        // Capture lifetime state before crossing the day boundary.
        let fade_after_day1 = ds.capacity_fade_fraction();
        let day_age_after_day1 = ds.day_age;

        // Verify day_age incremented.
        assert_eq!(
            day_age_after_day1, 1,
            "day_age should be 1 after first update_daily"
        );

        // Cross the boundary at SOC 0.5.
        ds.reset_day_tracking(0.5);

        // Daily tracking must be cleared.
        assert_eq!(
            ds.dod_max_today, 0.0,
            "dod_max_today must reset to 0 at day boundary"
        );
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
            (ds.capacity_fade_fraction() - fade_after_day1).abs() < 1e-15,
            "capacity_fade must be unchanged by reset_day_tracking: before={fade_after_day1:.10}, after={:.10}",
            ds.capacity_fade_fraction()
        );
        assert_eq!(
            ds.day_age, day_age_after_day1,
            "day_age must not change during reset_day_tracking"
        );

        // Day 2 aging must continue to accumulate -- q_li1 is monotonically increasing.
        // (capacity_fade itself is negative during the BOL transient.)
        let q_li1_after_day1 = ds.q_li1;
        ds.accumulate(dt_s, T_REF, V_REF, 0.5).unwrap();
        ds.update_daily(&u_neg, &RainflowCounter::default());
        assert!(
            ds.q_li1 >= q_li1_after_day1,
            "q_li1 after day 2 ({:.10}) must be >= day 1 ({q_li1_after_day1:.10})",
            ds.q_li1
        );
        assert_eq!(
            ds.day_age, 2,
            "day_age should be 2 after second update_daily"
        );
    }

    /// Regression test for the HARES 3-point residue rainflow algorithm on a
    /// 9-point SOC sequence derived from load amplitudes.
    ///
    /// This test validates the HARES implementation's specific output, NOT ASTM
    /// E1049-85 standard compliance.  The 3-point algorithm processes reversals
    /// left-to-right and extracts cycles differently from the full ASTM residue
    /// procedure.
    ///
    /// Sequence (9 SOC values): 0.222, 0.556, 0.111, 1.000, 0.333, 0.778, 0.000, 0.889, 0.222
    ///
    /// Tracing the algorithm step by step:
    ///   After push(0.111): reversals=[0.222,0.556,0.111] → half-cycle extracted,
    ///     range=0.556−0.222=0.334, count=0.5. reversals=[0.556,0.111]
    ///   After push(1.000): reversals=[0.556,0.111,1.000] → half-cycle extracted,
    ///     range=0.556−0.111=0.445, count=0.5. reversals=[0.111,1.000]
    ///   After push(0.333): reversals=[0.111,1.000,0.333] → range_x=0.667 < range_y=0.889, no extraction
    ///   After push(0.778): reversals=[0.111,1.000,0.333,0.778] → range_x=0.445 < range_y=0.667, no extraction
    ///   After push(0.000): reversals=[0.111,1.000,0.333,0.778,0.000] → full cycle extracted,
    ///     range=0.778−0.333=0.445, count=1.0. reversals=[0.111,1.000,0.000]
    ///     Then: reversals=[0.111,1.000,0.000] → half-cycle extracted,
    ///     range=1.000−0.111=0.889, count=0.5. reversals=[1.000,0.000]
    ///   After push(0.889): no extraction (range_x < range_y)
    ///   After push(0.222): no extraction (range_x < range_y)
    ///
    /// Extracted cycles: (0.334,0.5), (0.445,0.5), (0.445,1.0), (0.889,0.5)
    /// total_cycles = 0.5+0.5+1.0+0.5 = 2.5
    ///
    /// Exact sum_squared_dod is computed from the same f64 literals to match
    /// floating-point arithmetic exactly.
    #[test]
    fn rainflow_3point_residue_regression() {
        let soc_sequence: &[f64] = &[
            0.222, 0.556, 0.111, 1.000, 0.333, 0.778, 0.000, 0.889, 0.222,
        ];

        let mut rc = RainflowCounter::default();
        for &soc in soc_sequence {
            rc.push(soc);
        }

        // The 3-point method extracts 4 events totalling 2.5 cycle-weight.
        assert!(
            (rc.total_cycles() - 2.5).abs() < 1e-10,
            "3-point residue sequence: expected total_cycles = 2.5, got {}",
            rc.total_cycles()
        );

        // Compute the exact expected sum_squared_dod from the same f64 literals,
        // so floating-point rounding is identical to the algorithm.
        let r1 = 0.556_f64 - 0.222_f64; // first half-cycle range
        let r2 = 0.556_f64 - 0.111_f64; // second half-cycle range
        let r3 = 0.778_f64 - 0.333_f64; // full cycle range
        let r4 = 1.000_f64 - 0.111_f64; // half-cycle from residue collapse
        let expected_sum_sq_dod = 0.5 * r1 * r1 + 0.5 * r2 * r2 + 1.0 * r3 * r3 + 0.5 * r4 * r4;
        assert!(
            (rc.sum_squared_dod_daily() - expected_sum_sq_dod).abs() < 1e-12,
            "sum_squared_dod_daily: expected {expected_sum_sq_dod:.10}, got {:.10}",
            rc.sum_squared_dod_daily()
        );
    }

    /// Run 7 days of combined calendar + cycling aging and verify:
    ///   1. Cumulative fade equals the sum of daily increments (bookkeeping
    ///      invariant).
    ///   2. The usable capacity is min(QLi, Qneg): at BOL the
    ///      negative-electrode branch binds at ≈ +0.9 % above nameplate, so
    ///      capacity fade is negative while the Li branch sits higher still
    ///      (the b0 = 1.07 intercept) — the reference model's BOL state.
    ///   3. The day-1 Li-branch mechanism increments match Smith 2017
    ///      Eq. 4–7 exactly (checked against the mechanism state directly:
    ///      the binding branch hides the Li arithmetic from `fade`).
    ///
    /// Each day: one full cycle (DOD = 1.0) at 25 °C, SOC held at 0.5 (the
    /// cycles enter through the rainflow counter, so dod_max_today = 0).
    #[test]
    fn degradation_accumulate_multi_day() {
        let u_neg = make_u_neg_table();
        let dt_s = SECONDS_PER_DAY;
        let mut ds = DegradationState::default();

        // One full cycle per day: [0, 1, 0, 1] extracts one full cycle of
        // range 1.0 → Σ count·DOD² = 1.0 and Σ count·DOD^βc2 = 1.0.
        let mut rf = RainflowCounter::default();
        rf.push(0.0);
        rf.push(1.0);
        rf.push(0.0);
        rf.push(1.0);
        assert!((rf.sum_squared_dod_daily() - 1.0).abs() < 1e-12);

        let mut daily_fade: Vec<f64> = Vec::with_capacity(7);
        let mut prev_fade = 0.0_f64;
        let mut prev_q_li1 = 0.0_f64;
        let mut prev_q_li2 = 0.0_f64;
        let mut prev_q_li3 = 0.0_f64;

        for day in 0..7u32 {
            ds.accumulate(dt_s, T_REF, V_REF, 0.5).unwrap();
            ds.update_daily(&u_neg, &rf);

            let fade = ds.capacity_fade_fraction();

            // At BOL the negative-electrode branch binds above nameplate.
            assert!(
                fade < 0.0,
                "day {day}: capacity fade should be negative at BOL (capacity \
                 above nameplate, the reference model's min(QLi, Qneg) state), \
                 got {fade:.8}"
            );

            // The break-in loss is active — a positive, accumulating loss.
            assert!(
                ds.q_li3 > 0.0,
                "day {day}: q_li3 must be a positive break-in loss, got {:.8}",
                ds.q_li3
            );

            // Day-1 mechanism increments against Smith 2017 Eq. 4-7 at
            // T_REF, V_REF, dod_max = 0:
            //   dq_li1 = B1_REF·tafel_b1/√day_age (day_age = 1)
            //   dq_li2 = B2_REF·arr·√Σ(count·DOD²) = B2_REF
            //   dq_li3 = (b3_day − q_li3_prev)/τ with b3_day = B3_REF
            if day == 1 {
                let tafel_day1 = tafel_b1_factor(0.5, T_REF);
                let expected_dq_li1 = B1_REF * tafel_day1;
                assert!(
                    (ds.q_li1 - prev_q_li1 - expected_dq_li1).abs() < 1e-12,
                    "day-1 dq_li1: expected {expected_dq_li1:.10}, got {:.10}",
                    ds.q_li1 - prev_q_li1
                );
                assert!((ds.q_li2 - prev_q_li2 - B2_REF).abs() < 1e-12);
                let expected_dq_li3 = (B3_REF - prev_q_li3) / TAU_B3;
                assert!(
                    (ds.q_li3 - prev_q_li3 - expected_dq_li3).abs() < 1e-12,
                    "day-1 dq_li3: expected {expected_dq_li3:.10}, got {:.10}",
                    ds.q_li3 - prev_q_li3
                );
            }

            daily_fade.push(fade - prev_fade);
            prev_fade = fade;
            prev_q_li1 = ds.q_li1;
            prev_q_li2 = ds.q_li2;
            prev_q_li3 = ds.q_li3;
            ds.reset_day_tracking(0.5);
        }

        // The final capacity_fade must equal the sum of daily increments (bookkeeping invariant).
        let sum_of_increments: f64 = daily_fade.iter().sum();
        let final_fade = ds.capacity_fade_fraction();
        assert!(
            (final_fade - sum_of_increments).abs() < 1e-12,
            "final capacity_fade ({final_fade:.10}) must equal sum of daily increments ({sum_of_increments:.10})"
        );
    }

    /// Mechanism 3 (break-in loss): q_li3 converges toward the equilibrium
    /// set by the daily b3 integral (B3_REF at T_REF, dod_max = 0) with the
    /// exponential τ = TAU_B3 = 5-day time constant — monotonically
    /// increasing increments that shrink geometrically.
    #[test]
    fn degradation_break_in_loss_converges() {
        let u_neg = make_u_neg_table();
        let dt_s = SECONDS_PER_DAY;
        let mut ds = DegradationState::default();
        // No cycling: the break-in mechanism in isolation.
        let rf = RainflowCounter::default();

        let mut prev_dq_li3 = f64::INFINITY;
        let mut prev_q_li3 = 0.0_f64;

        for day in 0..30u32 {
            ds.accumulate(dt_s, T_REF, V_REF, 0.5).unwrap();
            ds.update_daily(&u_neg, &rf);
            let dq_li3 = ds.q_li3 - prev_q_li3;

            // The loss is positive and its increments are positive but
            // shrinking (exponential convergence).
            assert!(
                ds.q_li3 > 0.0,
                "day {day}: q_li3 must be a positive break-in loss, got {}",
                ds.q_li3
            );
            assert!(
                dq_li3 > 0.0,
                "day {day}: dq_li3 must be positive (loss accumulating), got {dq_li3:.8}"
            );
            if day > 0 {
                assert!(
                    dq_li3 < prev_dq_li3 + 1e-15,
                    "day {day}: increments must shrink ({} vs prev {prev_dq_li3:.8})",
                    dq_li3
                );
            }
            prev_dq_li3 = dq_li3;
            prev_q_li3 = ds.q_li3;
            ds.reset_day_tracking(0.5);
        }

        // Converged onto the daily integral: the remaining relaxation gap
        // is (1 − 1/τ)^30 of the equilibrium — far under 1 %.
        assert!(
            (ds.q_li3 - B3_REF).abs() < 0.01 * B3_REF,
            "q_li3 should converge to the b3 integral ({B3_REF}) by day 30, got {}",
            ds.q_li3
        );
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

        // Mechanism-2 state directly: through `fade` the min(QLi, Qneg)
        // structure mixes in the negative-electrode branch, whose own
        // negative activation energy (Ea_c2 = −48260) would blur the
        // ratio this test exists to pin.
        let cycling_25 = ds_25.q_li2;
        let cycling_0c = ds_0c.q_li2;

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
            (ratio - expected_ratio).abs() < 1e-9,
            "0C/25C cycling loss ratio should be exactly {expected_ratio:.4}, got {ratio:.4}"
        );
    }

    /// Run 30 days of calendar aging at 25 C with no cycling. The break-in
    /// loss (q_li3) must be a positive loss after day 1, never overshoot the
    /// day's b3 integral, and converge to it by day 30 (~6 τ).
    #[test]
    fn break_in_loss_q_li3_reaches_equilibrium() {
        let u_neg = make_u_neg_table();
        let mut ds = DegradationState::default();
        let cell_temp_k = 298.15; // 25 C
        let dt_s = 300.0; // 5-min steps
        let steps_per_day = 288;

        // Accumulate and update one day at a time (no cycling).
        for day in 0..30 {
            for _ in 0..steps_per_day {
                let v_oc = 3.8; // mid-SOC NMC
                ds.accumulate(dt_s, cell_temp_k, v_oc, 0.5).unwrap();
            }
            let b3_accum_before_update = ds.b3_accum;
            ds.update_daily(&u_neg, &RainflowCounter::default());

            // After day 1, the break-in loss is positive and underway.
            if day == 0 {
                assert!(
                    ds.q_li3 > 0.0,
                    "q_li3 should be a positive break-in loss after day 1, got {}",
                    ds.q_li3
                );
            }

            // The loss never overshoots the integral it relaxes toward
            // (after update_daily, b3_accum is reset for the next day, so
            // check against the value the update consumed).
            assert!(
                ds.q_li3 <= b3_accum_before_update + 1e-12,
                "day {day}: q_li3 ({}) must not overshoot the b3 integral \
                 ({b3_accum_before_update})",
                ds.q_li3
            );

            ds.reset_day_tracking(0.5);
        }

        // By day 30 (~6x TAU_B3 = 5 days), the loss has converged onto the
        // per-day integral (the relaxation remainder is e^-6 ≈ 0.25 %).
        let b3_day =
            0.02805_f64 * (0.0066_f64 * 96_485.0 / 8.314 * (3.8 / 298.15 - 3.7 / 298.15)).exp();
        assert!(
            (ds.q_li3 - b3_day).abs() < 0.01 * b3_day,
            "q_li3 should converge to the daily b3 integral ({b3_day:.6}) by \
             day 30, got {}",
            ds.q_li3
        );
    }

    // -----------------------------------------------------------------------
    // Midnight boundary ordering tests (T-0416)
    // -----------------------------------------------------------------------

    /// Helper: run N days of pure calendar aging at a fixed temperature and
    /// SOC, using a specified number of sub-daily timesteps.  Returns the
    /// final DegradationState.
    ///
    /// This mirrors the call ordering that `Battery::step()` uses
    /// post-fix: boundary check → update_daily → reset → accumulate.
    fn run_calendar_aging_steps(
        days: u32,
        steps_per_day: usize,
        cell_temp_k: f64,
        v_oc: f64,
        soc: f64,
    ) -> DegradationState {
        let u_neg = make_u_neg_table();
        let mut ds = DegradationState::default();
        ds.reset_day_tracking(soc);
        let dt_s = SECONDS_PER_DAY / steps_per_day as f64;
        for _ in 0..days {
            for _ in 0..steps_per_day {
                ds.accumulate(dt_s, cell_temp_k, v_oc, soc).unwrap();
            }
            ds.update_daily(&u_neg, &RainflowCounter::default());
            ds.reset_day_tracking(soc);
        }
        ds
    }

    /// The per-step accumulation ordering fix eliminates the 1/N_steps drift.
    ///
    /// Before the fix, the first timestep of each new day was accumulated into
    /// the previous day's b1/b2/b3 accumulators before update_daily() ran,
    /// then lost when update_daily() reset them.  The lost fraction was
    /// 1/steps_per_day of each day's accumulation.
    ///
    /// After the fix, update_daily() runs *before* the current step's
    /// accumulate(), so every step belongs to the correct day.  The cumulative
    /// q_li1 after N days must be independent of steps_per_day.
    ///
    /// This test runs 30 days of calendar aging at 25 °C with 1, 24, and 288
    /// steps/day and asserts that q_li1 is identical across all resolutions.
    #[test]
    fn calendar_aging_independent_of_steps_per_day() {
        let cell_temp_k = T_REF;
        let v_oc = V_REF;
        let soc = 0.5;
        let days = 30;

        let ds_1 = run_calendar_aging_steps(days, 1, cell_temp_k, v_oc, soc);
        let ds_24 = run_calendar_aging_steps(days, 24, cell_temp_k, v_oc, soc);
        let ds_288 = run_calendar_aging_steps(days, 288, cell_temp_k, v_oc, soc);

        let q1 = ds_1.q_li1;
        let q24 = ds_24.q_li1;
        let q288 = ds_288.q_li1;

        // All three must agree to within floating-point tolerance.  The
        // pre-fix bug would produce a ~0.35% deficit at 288 steps/day
        // relative to 1 step/day.
        assert!(
            (q1 - q288).abs() < q1.abs() * 1e-6,
            "q_li1 must be independent of steps_per_day: 1 step={q1:.10e}, 288 steps={q288:.10e}, diff={:.10e}",
            (q1 - q288).abs()
        );
        assert!(
            (q24 - q288).abs() < q24.abs() * 1e-6,
            "q_li1 must be independent of steps_per_day: 24 steps={q24:.10e}, 288 steps={q288:.10e}, diff={:.10e}",
            (q24 - q288).abs()
        );
    }

    /// The cumulative q_li1 after N days must equal the sum of independently
    /// computed per-day q_li1 increments.
    ///
    /// Each day's increment is computed using the same sqrt-of-time formula
    /// but tracking the running q_li1 from previous days.  The sum of these
    /// increments must match the N-day cumulative q_li1, proving that no
    /// timestep's contribution is lost or double-counted at the midnight
    /// boundary.
    #[test]
    fn cumulative_q_li1_equals_sum_of_daily_increments() {
        let u_neg = make_u_neg_table();
        let cell_temp_k = T_REF;
        let v_oc = V_REF;
        let soc = 0.5;
        let days = 10u32;
        let steps_per_day = 288usize;
        let dt_s = SECONDS_PER_DAY / steps_per_day as f64;

        // Compute per-day increments, tracking the running q_li1.
        // The Smith 2017 sqrt-of-time formula is path-dependent:
        //   dq = b1_eff / sqrt(day_age)        when q_li1 ≈ 0
        //   dq = 0.5 * b1_eff^2 / q_li1        when q_li1 > 0
        let mut running_q_li1 = 0.0_f64;
        let mut daily_increments: Vec<f64> = Vec::with_capacity(days as usize);
        for day in 0..days {
            // Simulate one day's accumulation in a fresh state with the
            // correct day_age and running q_li1.
            let mut ds = DegradationState {
                day_age: day,
                q_li1: running_q_li1,
                ..Default::default()
            };
            ds.reset_day_tracking(soc);
            for _ in 0..steps_per_day {
                ds.accumulate(dt_s, cell_temp_k, v_oc, soc).unwrap();
            }
            ds.update_daily(&u_neg, &RainflowCounter::default());
            let increment = ds.q_li1 - running_q_li1;
            daily_increments.push(increment);
            running_q_li1 = ds.q_li1;
        }

        // Run the full multi-day sequence in one DegradationState.
        let ds_cumulative = run_calendar_aging_steps(days, steps_per_day, cell_temp_k, v_oc, soc);

        let sum_increments: f64 = daily_increments.iter().sum();
        assert!(
            (ds_cumulative.q_li1 - sum_increments).abs() < ds_cumulative.q_li1.abs() * 1e-10,
            "cumulative q_li1 ({:.10e}) must equal sum of daily increments ({:.10e}), diff={:.10e}",
            ds_cumulative.q_li1,
            sum_increments,
            (ds_cumulative.q_li1 - sum_increments).abs()
        );
    }

    /// Reference validation: independently compute q_li1 for a multi-day
    /// calendar-aging sequence using the Smith 2017 Eq.2–4 formulas and
    /// compare against DegradationState's output.
    ///
    /// The reference computes each day's increment as:
    ///   dq_li1 = b1_eff / sqrt(day_age)   (first day, q_li1 ≈ 0)
    ///   dq_li1 = 0.5 * b1_eff² / q_li1    (subsequent days)
    ///
    /// where b1_eff = B1_REF * arr(T) * tafel(u_neg, T) * exp(gamma * dod^beta)
    ///
    /// At T_REF, dod=0, constant SOC=0.5: arr=1, dod_corr=1, so
    /// b1_eff = B1_REF * tafel_b1(0.5, T_REF).
    #[test]
    fn q_li1_matches_independent_smith2017_reference() {
        let cell_temp_k = T_REF;
        let v_oc = V_REF;
        let soc = 0.5;
        let days = 30u32;
        let steps_per_day = 288usize;

        // --- HARES DegradationState ---
        let ds = run_calendar_aging_steps(days, steps_per_day, cell_temp_k, v_oc, soc);

        // --- Independent reference computation ---
        // At T_REF, dod=0: arr=1, dod_corr=exp(gamma * 0^beta)=exp(0)=1.
        // b1_eff = B1_REF * tafel_b1(soc, T_REF).
        let tafel = tafel_b1_factor(soc, cell_temp_k);
        let b1_eff = B1_REF * tafel;

        let mut ref_q_li1 = 0.0_f64;
        for day in 0..days {
            let day_age = day as f64;
            let dq_li1 = if ref_q_li1.abs() < 1e-5 && day > 0 {
                b1_eff / day_age.sqrt()
            } else if ref_q_li1.abs() >= 1e-5 {
                0.5 * b1_eff.powi(2) / ref_q_li1
            } else {
                0.0 // day_age == 0: first day, skip
            };
            ref_q_li1 += dq_li1;
        }

        // The discrete integrator accumulates b1_accum over all timesteps,
        // then applies the Tafel correction once at midnight.  At T_REF with
        // constant temperature, b1_accum = B1_REF * 1.0 (full day) regardless
        // of step count, so the reference and HARES should agree closely.
        let rel_err = ((ds.q_li1 - ref_q_li1).abs() / ref_q_li1.abs()).abs();
        assert!(
            rel_err < 1e-6,
            "q_li1 HARES ({:.10e}) vs reference ({:.10e}): relative error {rel_err:.2e} must be < 1e-6",
            ds.q_li1,
            ref_q_li1
        );
    }

    /// The cell-temperature domain guard: a temperature outside the
    /// physically implausible envelope ([−100, +130] °C) errors loudly
    /// instead of being fed to the Arrhenius fit. Pre-fix, the EV's
    /// misattributed charger losses drove packs to 165–281 °C and the fit
    /// returned negative capacity fade silently.
    #[test]
    fn accumulate_rejects_cell_temperatures_outside_the_physical_envelope() {
        let mut ds = DegradationState::default();
        // 300 °C = 573.15 K — the conversion-loss-attribution excursion.
        let err = ds
            .accumulate(900.0, 573.15, V_REF, 0.5)
            .expect_err("573 K cell temperature must error, not degrade");
        let msg = err.to_string();
        assert!(
            msg.contains("outside the physically implausible envelope"),
            "error must name the envelope violation: {msg}"
        );
        // Symmetric cryogenic floor: −120 °C.
        assert!(ds.accumulate(900.0, 153.15, V_REF, 0.5).is_err());
        // In-envelope cold (−7 °C) is accepted — HARES deliberately operates
        // the fit below its 0 °C validated domain in cold climates, with the
        // extrapolation documented at the guard.
        assert!(ds.accumulate(900.0, 266.15, V_REF, 0.5).is_ok());
        // In-envelope hot (55 °C — the top of Smith 2017's tested range).
        assert!(ds.accumulate(900.0, 328.15, V_REF, 0.5).is_ok());
    }
}
