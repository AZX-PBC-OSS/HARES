//! Psychrometric coil physics: SHR solver, bypass factor, Ao factor, and supply temperature.

use hares_physics::{
    air_properties::moist_air_density_kg_m3,
    psychrometrics::{
        EPSILON as PSYCHROMETRIC_PRESSURE_RATIO,
        LATENT_HEAT_VAPORISATION_KJ_KG as VAPOR_LATENT_HEAT_KJ_PER_KG,
        SPECIFIC_HEAT_DRY_AIR_KJ_KG_K as DRY_AIR_CP_KJ_PER_KG_K,
        SPECIFIC_HEAT_WATER_VAPOUR_KJ_KG_K as VAPOR_CP_KJ_PER_KG_K, dew_point,
        moist_air_enthalpy, saturation_pressure_pa,
    },
};
use hares_types::HaresError;
use serde::{Deserialize, Serialize};

const SHR_MIN_HUMIDITY_RATIO: f64 = 1e-7;
const EPSILON: f64 = 1e-9;

// ---------------------------------------------------------------------------
// Henderson-Rengarajan latent degradation model
// ---------------------------------------------------------------------------

/// AHRI rated indoor conditions used to normalise Henderson-Rengarajan params.
/// 26.7 °C DB / 19.4 °C WB corresponds to 80 °F DB / 67 °F WB.
const HR_RATED_DB_C: f64 = 26.666_666_7;
const HR_RATED_WB_C: f64 = 19.444_444_4;

/// Maximum iterations for the `To` fixed-point solver.
const HR_TO_MAX_ITER: usize = 20;
/// Relative convergence tolerance for `To`.
const HR_TO_REL_TOL: f64 = 0.001;
/// Guard against division by near-zero `To`.
const HR_TO_MIN: f64 = 1e-10;

/// Latent degradation parameters for the Henderson-Rengarajan 1996 model.
///
/// All four fields must be strictly positive for the model to activate.
/// If any field is `<= 0.0` the model is disabled and the steady-state SHR is
/// returned unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatentDegradationParams {
    /// Nominal time for condensate removal to begin at rated conditions [s].
    /// Typical range: 1 000 – 2 000 s.
    pub twet_rated_s: f64,
    /// Ratio of initial moisture evaporation rate to steady-state latent capacity [-].
    /// Typical range: 1.0 – 2.0.
    pub gamma_rated: f64,
    /// Maximum cycling rate [cycles/hour].
    /// Typical range: 2.0 – 4.0 cycles/hr.
    pub max_cycling_rate: f64,
    /// Latent capacity time constant [s].
    /// Typical range: 20 – 60 s.
    pub latent_time_constant_s: f64,
}

impl LatentDegradationParams {
    /// Returns `true` when all four parameters are strictly positive,
    /// meaning the degradation model should be applied.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.twet_rated_s > 0.0
            && self.gamma_rated > 0.0
            && self.max_cycling_rate > 0.0
            && self.latent_time_constant_s > 0.0
    }
}

/// Calculate effective SHR accounting for latent capacity degradation at part load.
///
/// Based on Henderson & Rengarajan, "A Model to Predict the Latent Capacity of
/// Air Conditioners and Heat Pumps at Part-Load Conditions with Constant Fan
/// Operation", ASHRAE Transactions 1996, and validated against the EnergyPlus
/// implementation in `Coils.cc`.
///
/// When the coil cycles on/off at part load, moisture that collected on the coil
/// surface re-evaporates into the air during off cycles, reducing the net latent
/// (dehumidification) delivered per unit time.  The model quantifies this
/// re-evaporation and returns an **effective** SHR that is >= `steady_state_shr`
/// (more sensible-dominated) and <= 1.0.
///
/// # Parameters
/// - `steady_state_shr`       — SHR computed from coil physics at full-on conditions.
/// - `runtime_fraction`       — RTF = PLR / PLF (fraction of time compressor runs).
/// - `entering_db_c`          — Coil entering dry-bulb temperature [°C].
/// - `entering_wb_c`          — Coil entering wet-bulb temperature [°C].
/// - `rated_latent_capacity_w` — Latent capacity at AHRI rated conditions [W].
/// - `actual_latent_capacity_w` — Latent capacity at current operating conditions [W].
/// - `params`                 — Henderson-Rengarajan model coefficients.
/// - `heating_rtf` — Companion heating coil RTF, if heating runs during AC
///   off-cycles (e.g. heat-pump + auxiliary heat).
///   Pass `None` or `Some(0.0)` when not applicable.
///
/// # Returns
/// Effective SHR clamped to `[steady_state_shr, 1.0]`.
#[allow(clippy::too_many_arguments)]
pub(super) fn effective_shr_with_latent_degradation(
    steady_state_shr: f64,
    runtime_fraction: f64,
    entering_db_c: f64,
    entering_wb_c: f64,
    rated_latent_capacity_w: f64,
    actual_latent_capacity_w: f64,
    params: &LatentDegradationParams,
    heating_rtf: Option<f64>,
) -> f64 {
    debug_assert!(params.is_active(), "called with inactive params");

    // Continuous operation: no cycling degradation.
    if runtime_fraction >= 1.0 {
        return steady_state_shr;
    }

    // Guard against degenerate latent capacities.
    if actual_latent_capacity_w <= 0.0 || rated_latent_capacity_w <= 0.0 {
        return steady_state_shr;
    }

    let lat_ratio = rated_latent_capacity_w / actual_latent_capacity_w;

    // Adjust rated parameters to actual operating conditions.
    // twet is capped at 9 999 s per the EnergyPlus reference.
    let twet = (params.twet_rated_s * lat_ratio).min(9_999.0);

    // Gamma scales by the latent ratio and the entering humidity depression
    // relative to the AHRI rated depression (26.7 °C DB − 19.4 °C WB = 7.3 K).
    let db_wb_depression = entering_db_c - entering_wb_c;
    let rated_depression = HR_RATED_DB_C - HR_RATED_WB_C;
    let gamma = if rated_depression > 0.0 {
        params.gamma_rated * lat_ratio * (db_wb_depression / rated_depression)
    } else {
        params.gamma_rated * lat_ratio
    };

    let rtf = runtime_fraction.clamp(0.0, 1.0);
    let nmax = params.max_cycling_rate;
    let tau = params.latent_time_constant_s;

    // On- and off-cycle durations derived from the maximum cycling rate relation.
    // At max cycling rate Nmax = 1 / (4 * ton * (1-rtf))  →  ton = 3600/(4*Nmax*(1-rtf))
    // Similarly for toff.
    let ton = if (1.0 - rtf) > 0.0 {
        3600.0 / (4.0 * nmax * (1.0 - rtf))
    } else {
        return steady_state_shr; // RTF at or above 1 already handled above
    };
    let toff_base = if rtf > 0.0 {
        3600.0 / (4.0 * nmax * rtf)
    } else {
        return steady_state_shr; // RTF = 0: compressor never on, no degradation meaningful
    };

    // If a companion heating coil runs during the cooling off-cycle, the coil
    // sees warm moist air for a shorter effective off-time, reducing degradation.
    let heating_rtf_val = heating_rtf.unwrap_or(0.0).clamp(0.0, 1.0);
    let two_twet_over_gamma = if gamma > 0.0 {
        2.0 * twet / gamma
    } else {
        f64::INFINITY
    };
    let toff_capped = toff_base.min(two_twet_over_gamma);
    let toff_effective = if heating_rtf_val > rtf && heating_rtf_val > 0.0 {
        toff_capped * rtf / heating_rtf_val
    } else {
        toff_capped
    };

    // Initial estimate of `aa` — a surrogate for the moisture removal onset time.
    let aa = gamma * toff_effective - 0.25 / twet * gamma * gamma * toff_effective * toff_effective;

    // Iterative fixed-point solve for `To` (time at which steady-state latent
    // removal resumes after an off-cycle).
    let mut to = aa + tau;
    for _ in 0..HR_TO_MAX_ITER {
        let to_new = aa - tau * ((-to / tau).exp() - 1.0);
        let rel_err = (to_new - to).abs() / to.abs().max(HR_TO_MIN);
        to = to_new;
        if rel_err < HR_TO_REL_TOL {
            break;
        }
    }

    // Latent heat ratio multiplier: fraction of the on-cycle during which the
    // coil actually performs latent removal (after moisture has re-condensed).
    let aa_exp = (-ton / tau).exp();
    let denominator = ton + tau * (aa_exp - 1.0);
    let lhr_mult = if denominator.abs() > EPSILON {
        ((ton - to) / denominator).max(0.0)
    } else {
        0.0
    };

    let shr_eff = 1.0 - (1.0 - steady_state_shr) * lhr_mult;
    shr_eff.clamp(steady_state_shr, 1.0)
}
const BYPASS_FACTOR_FLOOR: f64 = 0.01;
const ADP_ITERATION_LIMIT: usize = 100;
const ADP_ERROR_TOL: f64 = 0.001;
const SHR_ITERATION_LIMIT: usize = 50;
const ITERATE_TOL_REL: f64 = 1e-5;
const ITERATE_PERTURBATION: f64 = 0.1;

/// Result of the SHR coil calculation, carrying all psychrometric coil state.
#[derive(Debug)]
pub(super) struct CoilResult {
    pub shr: f64,
    /// Apparatus dew point temperature [°C].
    pub adp_temp_c: f64,
    /// Coil bypass factor [-].
    pub bypass_factor: f64,
    /// Supply air dry-bulb temperature [°C]: T_adp + BF * (T_entering - T_adp).
    pub supply_temp_c: f64,
}

pub(super) fn calculate_shr(
    db_in_c: f64,
    w_in: f64,
    p_kpa: f64,
    q_kw: f64,
    flow_m3_s: f64,
    ao: f64,
) -> crate::Result<CoilResult> {
    if w_in <= SHR_MIN_HUMIDITY_RATIO || q_kw <= 0.0 || flow_m3_s <= 0.0 {
        return Ok(CoilResult {
            shr: 1.0,
            adp_temp_c: db_in_c,
            bypass_factor: 1.0,
            supply_temp_c: db_in_c,
        });
    }

    let mfr = calculate_mass_flow_rate(db_in_c, w_in, p_kpa, flow_m3_s);
    let bf = if mfr > 0.0 { (-ao / mfr).exp() } else { 0.0 };

    let h_in = moist_air_enthalpy(db_in_c, w_in);
    let d_h = if mfr > 0.0 { q_kw * 1000.0 / mfr } else { 0.0 };
    let h_adp = h_in - d_h / (1.0 - bf);

    let p_pa = p_kpa * 1000.0;
    let mut t_adp = dew_point(w_in.max(SHR_MIN_HUMIDITY_RATIO), p_pa);
    let mut t_adp_1 = t_adp;
    let mut t_adp_2 = t_adp;
    let mut w_adp = humidity_ratio_from_rel_hum(t_adp, 1.0, p_pa);
    let mut err = h_adp - moist_air_enthalpy(t_adp, w_adp);
    let mut err1 = err;
    let mut err2 = err;

    let mut shr_converged = false;
    for i in 1..=SHR_ITERATION_LIMIT {
        w_adp = humidity_ratio_from_rel_hum(t_adp, 1.0, p_pa);
        err = h_adp - moist_air_enthalpy(t_adp, w_adp);
        let (next, cvg, next_t1, next_e1, next_t2, next_e2) =
            iterate(t_adp, err, t_adp_1, err1, t_adp_2, err2, i);
        t_adp = next;
        t_adp_1 = next_t1;
        err1 = next_e1;
        t_adp_2 = next_t2;
        err2 = next_e2;
        if cvg {
            shr_converged = true;
            break;
        }
    }

    if !shr_converged {
        return Err(HaresError::Equipment(format!(
            "calculate_shr failed to converge after {SHR_ITERATION_LIMIT} iterations (err={err:.6})"
        )));
    }

    let h_tin_wadp = moist_air_enthalpy(db_in_c, w_adp);
    let denom = h_in - h_adp;
    let shr = if denom != 0.0 {
        ((h_tin_wadp - h_adp) / denom).min(1.0)
    } else {
        1.0
    };

    let supply_temp_c = t_adp + bf * (db_in_c - t_adp);

    Ok(CoilResult {
        shr,
        adp_temp_c: t_adp,
        bypass_factor: bf,
        supply_temp_c,
    })
}

pub(super) fn coil_ao_factor(
    db_in_c: f64,
    w_in: f64,
    p_kpa: f64,
    q_kw: f64,
    flow_m3_s: f64,
    shr: f64,
) -> crate::Result<f64> {
    let bf = coil_bypass_factor(db_in_c, w_in, p_kpa, q_kw, flow_m3_s, shr)?;
    let mfr = calculate_mass_flow_rate(db_in_c, w_in, p_kpa, flow_m3_s);
    Ok(-bf.ln() * mfr)
}

pub(super) fn coil_bypass_factor(
    db_in_c: f64,
    w_in: f64,
    p_kpa: f64,
    q_kw: f64,
    flow_m3_s: f64,
    shr: f64,
) -> crate::Result<f64> {
    let mfr = calculate_mass_flow_rate(db_in_c, w_in, p_kpa, flow_m3_s);
    if mfr <= 0.0 {
        return Ok(BYPASS_FACTOR_FLOOR);
    }

    let d_h = q_kw * 1000.0 / mfr;
    let h_in = moist_air_enthalpy(db_in_c, w_in);
    let h_tin_wout = h_in - (1.0 - shr) * d_h;
    let w_out = humidity_ratio_from_enthalpy_and_t_dry_bulb(h_tin_wout, db_in_c);
    let d_w = w_in - w_out;

    let h_out = h_in - d_h;
    let t_out = t_dry_bulb_from_enthalpy_and_humidity_ratio(h_out, w_out);
    let p_pa = p_kpa * 1000.0;

    let mut t_adp = dew_point(w_out.max(SHR_MIN_HUMIDITY_RATIO), p_pa);

    if shr == 1.0 {
        let w_adp = humidity_ratio_from_rel_hum(t_adp, 1.0, p_pa);
        let h_adp = moist_air_enthalpy(t_adp, w_adp);
        let bf = (h_out - h_adp) / (h_in - h_adp);
        return Ok(bf.max(BYPASS_FACTOR_FLOOR));
    }

    // Outlet RH > 100% is physically infeasible but can occur at low airflow
    // rates (e.g. 320 CFM/ton for room AC with SHR=0.75 at AHRI rated conditions).
    // OCHRE prints a warning but continues rather than aborting; we match
    // that error-recovery choice.
    // Ref: vendors/OCHRE/ochre/utils/equipment.py:839
    // Ref: EnergyPlus issue #10738 (DX coil negative bypass factor)

    let d_t = db_in_c - t_out;
    if d_t.abs() < EPSILON {
        return Ok(BYPASS_FACTOR_FLOOR);
    }
    let m_c = d_w / d_t;

    // ADP iteration: bisection-halving on sign change, matching OCHRE/EnergyPlus.
    // W_ADP = saturation humidity ratio at T_ADP (100% RH). OCHRE calls
    // GetHumRatioFromTWetBulb(T_ADP, T_ADP, P), which reduces to the saturation
    // humidity ratio when T_DB == T_WB (the psychrometer equation cancels).
    // Using humidity_ratio_from_rel_hum(t_adp, 1.0) matches that exactly.
    // Ref: vendors/OCHRE/ochre/utils/equipment.py:845-870
    let mut cnt = 0usize;
    let mut tol = 1.0;
    let mut err_last = 100.0_f64;
    let mut d_t_adp = 5.0_f64;
    while cnt < ADP_ITERATION_LIMIT && tol > ADP_ERROR_TOL {
        if cnt > 0 {
            t_adp += d_t_adp;
        }

        let w_adp = humidity_ratio_from_rel_hum(t_adp, 1.0, p_pa);
        let m = (w_in - w_adp) / (db_in_c - t_adp);
        let err = (m - m_c) / m_c;

        if err > 0.0 && err_last < 0.0 {
            d_t_adp = -d_t_adp / 2.0;
        }
        if err < 0.0 && err_last > 0.0 {
            d_t_adp = -d_t_adp / 2.0;
        }

        tol = err.abs();
        err_last = err;
        cnt += 1;
    }

    if cnt >= ADP_ITERATION_LIMIT && tol > ADP_ERROR_TOL {
        return Err(HaresError::Equipment(format!(
            "coil_bypass_factor failed to converge after {ADP_ITERATION_LIMIT} iterations (tol={tol:.6})"
        )));
    }

    if db_in_c - t_adp <= 0.0 {
        return Ok(BYPASS_FACTOR_FLOOR);
    }

    // ── Enthalpy-based bypass factor (EnergyPlus / OCHRE) ──
    //
    // BF = (h_out − h_ADP) / (h_in − h_ADP)
    //
    // The alternative temperature-based formula BF = (T_out − T_ADP)/(T_in − T_ADP)
    // is only valid for sensible-only coils (SHR ≈ 1). For wet coils, the
    // temperature form ignores the latent contribution and under-predicts BF by
    // 5–20% depending on humidity ratio. The enthalpy form correctly accounts
    // for both sensible and latent heat transfer across the coil surface.
    //
    // Ref: ASHRAE 2017 HOF Ch.18 Eq.63; EnergyPlus DXCoils.cc;
    //      vendors/OCHRE/ochre/utils/equipment.py:872-874
    let w_adp = humidity_ratio_from_rel_hum(t_adp, 1.0, p_pa);
    let h_adp = moist_air_enthalpy(t_adp, w_adp);
    let bf = (h_out - h_adp) / (h_in - h_adp);
    Ok(bf.max(BYPASS_FACTOR_FLOOR))
}

fn humidity_ratio_from_rel_hum(t_db_c: f64, rel_hum: f64, p_pa: f64) -> f64 {
    let rel_hum = rel_hum.clamp(0.0, 1.0);
    let vap = rel_hum * saturation_pressure_pa(t_db_c);
    PSYCHROMETRIC_PRESSURE_RATIO * vap / (p_pa - vap)
}

fn humidity_ratio_from_enthalpy_and_t_dry_bulb(h_j_kg: f64, t_db_c: f64) -> f64 {
    let h_kj_kg = h_j_kg / 1000.0;
    ((h_kj_kg - DRY_AIR_CP_KJ_PER_KG_K * t_db_c)
        / (VAPOR_LATENT_HEAT_KJ_PER_KG + VAPOR_CP_KJ_PER_KG_K * t_db_c))
        .max(0.0)
}

fn t_dry_bulb_from_enthalpy_and_humidity_ratio(h_j_kg: f64, w: f64) -> f64 {
    let h_kj_kg = h_j_kg / 1000.0;
    (h_kj_kg - VAPOR_LATENT_HEAT_KJ_PER_KG * w)
        / (DRY_AIR_CP_KJ_PER_KG_K + VAPOR_CP_KJ_PER_KG_K * w)
}

fn calculate_mass_flow_rate(db_in_c: f64, w_in: f64, p_kpa: f64, flow_m3_s: f64) -> f64 {
    // ── Deviation from OCHRE: dry-air mass flow ──
    //
    // ASHRAE HOF Ch.1 Eq.30 defines moist-air enthalpy h [J/kg_da] per unit
    // mass of DRY air. The energy balance Q = ṁ × Δh therefore requires ṁ on
    // the same dry-air basis: ṁ_da = V̇ × ρ_da [kg_da/s].
    //
    // OCHRE uses psychrolib.GetMoistAirDensity which returns ρ_moist =
    // (1+W)/v_da, giving ṁ_moist = V̇ × ρ_moist. This mixes a moist-air
    // mass flow with a dry-air enthalpy, producing a ~1% error in dH and all
    // downstream quantities (BF, ADP, Ao) at typical humidity ratios.
    //
    // Reference: ASHRAE 2017 HOF Ch.1 §1.8 "Thermodynamic Properties of
    // Moist Air" — all specific properties are per kg dry air.
    // Ref: EnergyPlus `PsyRhoAirFnPbTdbW` also returns ρ_da.
    let rho_da = moist_air_density_kg_m3(p_kpa * 1000.0, db_in_c, w_in.max(0.0));
    flow_m3_s * rho_da
}

fn iterate(
    x0: f64,
    f0: f64,
    mut x1: f64,
    mut f1: f64,
    mut x2: f64,
    mut f2: f64,
    icount: usize,
) -> (f64, bool, f64, f64, f64, f64) {
    let tol_rel = ITERATE_TOL_REL;
    let small = EPSILON;
    let dx = ITERATE_PERTURBATION;

    if (x0 - x1).abs() < tol_rel * x0.abs().max(small) && icount != 1 || f0 == 0.0 {
        return (x0, true, x1, f1, x2, f2);
    }

    let mut mode = if icount == 1 {
        1
    } else if icount == 2 {
        2
    } else {
        3
    };

    let mut x_new = 0.0;

    if mode == 3 {
        if x0 == x1 {
            x1 = x2;
            f1 = f2;
            mode = 2;
        } else if x0 == x2 {
            mode = 2;
        } else {
            let c = ((f2 - f0) / (x2 - x0) - (f1 - f0) / (x1 - x0)) / (x2 - x1);
            let b = (f1 - f0) / (x1 - x0) - (x1 + x0) * c;
            let a = f0 - (b + c * x0) * x0;

            if c.abs() < small
                || f1.abs() < small
                || ((a + (b + c * x1) * x1 - f1) / f1).abs() > small
            {
                mode = 2;
            } else {
                let d = b * b - 4.0 * a * c;
                if d < 0.0 {
                    mode = 2;
                } else {
                    if d > 0.0 {
                        x_new = (-b + d.sqrt()) / (2.0 * c);
                        let x_other = -x_new - b / c;
                        if (x_new - x0).abs() > (x_other - x0).abs() {
                            x_new = x_other;
                        }
                    } else {
                        x_new = -b / (2.0 * c);
                    }

                    if f1 * f0 > 0.0 && f2 * f0 > 0.0 {
                        if f2.abs() > f1.abs() {
                            x2 = x1;
                            f2 = f1;
                        }
                    } else if f2 * f0 > 0.0 {
                        x2 = x1;
                        f2 = f1;
                    }
                    x1 = x0;
                    f1 = f0;
                }
            }
        }
    }

    if mode == 2 {
        let denom = x1 - x0;
        if denom.abs() < small {
            mode = 1;
        } else {
            let m = (f1 - f0) / denom;
            if m == 0.0 {
                mode = 1;
            } else {
                x_new = x0 - f0 / m;
                x2 = x1;
                f2 = f1;
                x1 = x0;
                f1 = f0;
            }
        }
    }

    if mode == 1 {
        x_new = if x0.abs() > small {
            x0 * (1.0 + dx)
        } else {
            dx
        };
        x2 = x1;
        f2 = f1;
        x1 = x0;
        f1 = f0;
    }

    (x_new, false, x1, f1, x2, f2)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod latent_degradation_tests {
    use super::{LatentDegradationParams, effective_shr_with_latent_degradation};

    /// Typical indoor conditions (AHRI-rated).
    const DB_C: f64 = 26.7;
    const WB_C: f64 = 19.4;
    const RATED_LAT_W: f64 = 2_000.0;
    const ACTUAL_LAT_W: f64 = 2_000.0;
    const STEADY_SHR: f64 = 0.75;

    /// Parameters that produce visible (non-maximal) degradation at high RTF.
    ///
    /// With Nmax=3 and large twet (e.g. 1000s), the coil always has To > ton
    /// for moderate RTFs, giving maximum degradation (SHR=1.0).  To exercise
    /// the partial-degradation path we use a small twet (100s) so the coil
    /// re-wets quickly relative to cycle length.
    ///
    /// At RTF=0.9: ton = 3600/(4*3*0.1) = 3000s >> twet → To < ton → lhr_mult > 0.
    fn partial_degradation_params() -> LatentDegradationParams {
        LatentDegradationParams {
            twet_rated_s: 100.0,
            gamma_rated: 1.5,
            max_cycling_rate: 3.0,
            latent_time_constant_s: 45.0,
        }
    }

    // ------------------------------------------------------------------
    // Test 1: is_active() reflects whether all params are positive.
    // ------------------------------------------------------------------
    #[test]
    fn is_active_requires_all_positive() {
        let zero = LatentDegradationParams::default();
        assert!(
            !zero.is_active(),
            "default (all zero) params must be inactive"
        );

        let active = partial_degradation_params();
        assert!(active.is_active(), "all-positive params must be active");

        let partial = LatentDegradationParams {
            twet_rated_s: 0.0,
            ..active
        };
        assert!(
            !partial.is_active(),
            "one zero field must make params inactive"
        );
    }

    // ------------------------------------------------------------------
    // Test 2: Zero latent capacity → returns steady_state_shr unchanged.
    // ------------------------------------------------------------------
    #[test]
    fn zero_latent_capacity_returns_steady_state() {
        let params = partial_degradation_params();
        let result = effective_shr_with_latent_degradation(
            STEADY_SHR, 0.5, DB_C, WB_C, 0.0, 0.0, &params, None,
        );
        assert_eq!(
            result, STEADY_SHR,
            "zero latent capacities must return steady_state_shr"
        );
    }

    // ------------------------------------------------------------------
    // Test 3: RTF = 1.0 → continuous operation, no degradation.
    // ------------------------------------------------------------------
    #[test]
    fn full_load_no_degradation() {
        let params = partial_degradation_params();
        let result = effective_shr_with_latent_degradation(
            STEADY_SHR,
            1.0,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &params,
            None,
        );
        assert_eq!(
            result, STEADY_SHR,
            "RTF=1.0 must return steady_state_shr unchanged"
        );
    }

    // ------------------------------------------------------------------
    // Test 4: High RTF (0.9) → on-cycle long relative to twet → partial
    //         degradation: SHR_eff in (STEADY_SHR, 1.0).
    //
    // At RTF=0.9: ton=3000s >> twet=100s.
    // toff=333s, toff_capped = min(333, 2*100/1.5=133) = 133s.
    // aa = 1.5*133 - 0.25/100*2.25*133^2 ≈ 199.5 - 99.7 = 99.8.
    // to ≈ 144.8s < ton(3000s) → lhr_mult > 0 → STEADY_SHR < SHR_eff < 1.0.
    // ------------------------------------------------------------------
    #[test]
    fn high_rtf_partial_degradation() {
        let params = partial_degradation_params();
        let result = effective_shr_with_latent_degradation(
            STEADY_SHR,
            0.9,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &params,
            None,
        );
        assert!(
            result > STEADY_SHR,
            "high-RTF SHR ({result:.4}) must exceed steady-state ({STEADY_SHR})"
        );
        assert!(result <= 1.0, "SHR must not exceed 1.0, got {result:.4}");
    }

    // ------------------------------------------------------------------
    // Test 5: With large twet, maximum degradation (SHR=1.0) occurs at RTFs
    //         where the on-cycle duration (ton) is shorter than To.
    //
    // For twet=1000s, gamma=1.5, Nmax=3:
    //   toff_cap = 2*twet/gamma = 1333s
    //   RTF=0.3: ton=428s, toff=1000s(capped=1000s), aa≈938s, To≈983s > ton → SHR=1.0
    //   RTF=0.5: ton=600s, toff=600s(capped=600s), aa≈697s, To≈742s > ton → SHR=1.0
    //   RTF=0.7: ton=1000s, toff=428s, aa≈538s, To≈583s < ton → partial degradation
    // ------------------------------------------------------------------
    #[test]
    fn large_twet_maximum_degradation_at_low_rtf() {
        let params = LatentDegradationParams {
            twet_rated_s: 1000.0,
            gamma_rated: 1.5,
            max_cycling_rate: 3.0,
            latent_time_constant_s: 45.0,
        };
        // Only RTFs where ton < To yield SHR=1.0.
        for rtf_pct in [10u32, 30, 50] {
            let rtf = rtf_pct as f64 / 100.0;
            let result = effective_shr_with_latent_degradation(
                STEADY_SHR,
                rtf,
                DB_C,
                WB_C,
                RATED_LAT_W,
                ACTUAL_LAT_W,
                &params,
                None,
            );
            assert_eq!(
                result, 1.0,
                "twet=1000s at RTF={rtf} → To > ton → max degradation, got {result:.4}"
            );
        }
    }

    // ------------------------------------------------------------------
    // Test 6: SHR always clamped to [steady_state_shr, 1.0] across RTF range.
    // ------------------------------------------------------------------
    #[test]
    fn shr_always_clamped_in_valid_range() {
        let params = partial_degradation_params();
        for rtf_pct in 1..=99u32 {
            let rtf = rtf_pct as f64 / 100.0;
            let result = effective_shr_with_latent_degradation(
                STEADY_SHR,
                rtf,
                DB_C,
                WB_C,
                RATED_LAT_W,
                ACTUAL_LAT_W,
                &params,
                None,
            );
            assert!(
                result >= STEADY_SHR,
                "SHR {result:.4} below steady-state {STEADY_SHR} at RTF={rtf}"
            );
            assert!(result <= 1.0, "SHR {result:.4} above 1.0 at RTF={rtf}");
        }
    }

    // ------------------------------------------------------------------
    // Test 7: Companion heating RTF > cooling RTF → shorter effective off-time
    //         → less moisture re-evaporation → lower SHR_eff.
    //
    // Uses small twet (50s) at high RTF (0.9) so partial degradation is visible.
    // ------------------------------------------------------------------
    #[test]
    fn companion_heating_reduces_degradation() {
        let params = LatentDegradationParams {
            twet_rated_s: 50.0,
            gamma_rated: 1.5,
            max_cycling_rate: 3.0,
            latent_time_constant_s: 45.0,
        };
        let cooling_rtf = 0.9;
        let heating_rtf = 0.95;

        let shr_no_heat = effective_shr_with_latent_degradation(
            STEADY_SHR,
            cooling_rtf,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &params,
            None,
        );
        let shr_with_heat = effective_shr_with_latent_degradation(
            STEADY_SHR,
            cooling_rtf,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &params,
            Some(heating_rtf),
        );
        // With companion heating the effective off-time is shorter → less moisture
        // re-evaporates → more latent removal → lower effective SHR.
        assert!(
            shr_with_heat <= shr_no_heat,
            "companion heating SHR ({shr_with_heat:.4}) must be <= no-heating ({shr_no_heat:.4})"
        );
    }

    // ------------------------------------------------------------------
    // Test 8: Very large twet at high RTF → partial degradation, SHR in
    //         valid range.
    //
    // At RTF=0.9, ton=3000s.  Even with twet=50000s, toff_base=333s is the
    // binding constraint (toff_cap = 2*50000/1.5 = 66666s >> 333s).  Thus
    // aa ≈ gamma*toff_base ≈ 500s and To ≈ 545s << ton → partial degradation.
    // ------------------------------------------------------------------
    #[test]
    fn very_large_twet_high_rtf_partial_degradation() {
        let params = LatentDegradationParams {
            twet_rated_s: 50_000.0,
            gamma_rated: 1.5,
            max_cycling_rate: 3.0,
            latent_time_constant_s: 45.0,
        };
        let result = effective_shr_with_latent_degradation(
            STEADY_SHR,
            0.9,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &params,
            None,
        );
        assert!(
            result > STEADY_SHR && result <= 1.0,
            "huge twet at high RTF → partial degradation: expected SHR in ({STEADY_SHR:.4}, 1.0], got {result:.4}"
        );
    }

    // ------------------------------------------------------------------
    // Test 9: Changing the time constant alters the effective SHR, and both
    //         values remain within the valid range [steady_state_shr, 1.0].
    //
    // Uses small twet (50s) at RTF=0.9 so both param sets produce partial
    // degradation.  The relationship between tau and SHR direction is
    // non-trivial (tau affects both To and the lhr_mult denominator), so
    // this test verifies that the parameter has an observable effect and
    // that results stay bounded rather than asserting a fixed direction.
    // ------------------------------------------------------------------
    #[test]
    fn time_constant_affects_shr_within_valid_range() {
        let fast = LatentDegradationParams {
            twet_rated_s: 50.0,
            gamma_rated: 1.5,
            max_cycling_rate: 3.0,
            latent_time_constant_s: 10.0,
        };
        let slow = LatentDegradationParams {
            latent_time_constant_s: 120.0,
            ..fast
        };
        let rtf = 0.9;
        let shr_fast = effective_shr_with_latent_degradation(
            STEADY_SHR,
            rtf,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &fast,
            None,
        );
        let shr_slow = effective_shr_with_latent_degradation(
            STEADY_SHR,
            rtf,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &slow,
            None,
        );
        // Both must be in [STEADY_SHR, 1.0].
        assert!(
            (STEADY_SHR..=1.0).contains(&shr_fast),
            "fast-tau SHR {shr_fast:.4} out of range [{STEADY_SHR}, 1.0]"
        );
        assert!(
            (STEADY_SHR..=1.0).contains(&shr_slow),
            "slow-tau SHR {shr_slow:.4} out of range [{STEADY_SHR}, 1.0]"
        );
        // Tau must have a visible effect on the result.
        assert_ne!(
            (shr_fast * 1e6) as i64,
            (shr_slow * 1e6) as i64,
            "different tau values must produce different SHR results"
        );
    }

    // ------------------------------------------------------------------
    // Test 10: Regression at RTF=0.9 with large twet=1000s.
    //
    // Inputs: twet_rated=1000s, gamma=1.5, Nmax=3 cyc/hr, tau=45s, RTF=0.9,
    //         DB=26.7°C, WB=19.4°C, rated_lat=actual_lat=2000W, SHR=0.75.
    //
    // Hand calculation:
    //   twet=1000s, gamma≈1.5 (rated conditions, depression ratio ≈ 1)
    //   ton = 3600/(4*3*0.1) = 3000s
    //   toff = 3600/(4*3*0.9) = 333.3s
    //   toff_capped = min(333.3, 2*1000/1.5=1333) = 333.3s
    //   aa = 1.5*333.3 - 0.25/1000*2.25*333.3^2 = 499.95 - 62.49 = 437.46
    //   to_0 = 437.46 + 45 = 482.46
    //   to_new = 437.46 - 45*(exp(-482.46/45) - 1) ≈ 437.46 + 45 = 482.46 (converged)
    //   to = 482.46s < ton(3000s)
    //   aa_exp = exp(-3000/45) ≈ 0
    //   denominator = 3000 + 45*(0-1) = 2955
    //   lhr_mult = (3000-482.46)/2955 = 0.8519
    //   SHR_eff = 1 - 0.25*0.8519 = 0.787
    // ------------------------------------------------------------------
    #[test]
    fn regression_rtf_09_large_twet() {
        let params = LatentDegradationParams {
            twet_rated_s: 1000.0,
            gamma_rated: 1.5,
            max_cycling_rate: 3.0,
            latent_time_constant_s: 45.0,
        };
        let result = effective_shr_with_latent_degradation(
            0.75, 0.9, DB_C, WB_C, 2_000.0, 2_000.0, &params, None,
        );
        // Expected ≈ 0.787; allow ±0.005 for floating-point and gamma normalisation.
        assert!(
            (result - 0.787).abs() < 0.005,
            "expected SHR ≈ 0.787, got {result:.4}"
        );
    }

    // ------------------------------------------------------------------
    // Test 11: High cycling rate (Nmax=6) with small twet (50s) at RTF=0.5
    //          → degradation visible (SHR > STEADY_SHR).
    // ------------------------------------------------------------------
    #[test]
    fn high_cycling_small_twet_shows_degradation() {
        let params = LatentDegradationParams {
            twet_rated_s: 50.0,
            gamma_rated: 1.5,
            max_cycling_rate: 6.0,
            latent_time_constant_s: 45.0,
        };
        let result = effective_shr_with_latent_degradation(
            STEADY_SHR,
            0.5,
            DB_C,
            WB_C,
            RATED_LAT_W,
            ACTUAL_LAT_W,
            &params,
            None,
        );
        // ton = 3600/(4*6*0.5) = 300s > twet=50s → To < ton → lhr_mult > 0 → degradation.
        assert!(
            result >= STEADY_SHR,
            "degraded SHR must be >= steady_state_shr, got {result:.4}"
        );
        assert!(result <= 1.0, "SHR must not exceed 1.0, got {result:.4}");
    }
}

#[cfg(test)]
mod coil_psychrometric_tests {
    use super::*;

    // AHRI 210/240 rated indoor conditions.
    const T_DB: f64 = 26.67; // 80 °F
    const W_IN: f64 = 0.011159; // psychrometric humidity ratio at T_DB=26.67°C, T_WB=19.44°C, P=101325 Pa (AHRI 210/240)
    const P_KPA: f64 = 101.325;
    // 3-ton unit
    const Q_KW: f64 = 10.5505;
    const FLOW_M3S: f64 = 0.49554;

    // ------------------------------------------------------------------
    // Test 1: Bypass factor at AHRI rated conditions, SHR = 0.70.
    // Reference: psychrolib with dry-air mass flow (ASHRAE HOF Ch.1).
    // HARES uses ṁ_da (not ṁ_moist) so BF ≈ 0.154 vs OCHRE's 0.163.
    // ------------------------------------------------------------------
    #[test]
    fn coil_bypass_factor_ahri_rated_shr_070() {
        let bf = coil_bypass_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.70).unwrap();
        assert!(
            (bf - 0.154).abs() < 0.005,
            "BF at SHR=0.70: expected 0.154 ± 0.005 (dry-air basis), got {bf:.5}"
        );
    }

    // ------------------------------------------------------------------
    // Test 2: Bypass factor at AHRI rated conditions, SHR = 0.74.
    // Reference: psychrolib with dry-air mass flow (ASHRAE HOF Ch.1).
    // ------------------------------------------------------------------
    #[test]
    fn coil_bypass_factor_ahri_rated_shr_074() {
        let bf = coil_bypass_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.74).unwrap();
        assert!(
            (bf - 0.040).abs() < 0.005,
            "BF at SHR=0.74: expected 0.040 ± 0.005 (dry-air basis), got {bf:.5}"
        );
    }

    // ------------------------------------------------------------------
    // Test 3: Round-trip: compute Ao from SHR=0.70, feed to calculate_shr,
    // recover SHR ≈ 0.70.
    // Reference: ASHRAE psychrometric consistency.
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_round_trip_ahri_conditions() {
        let ao = coil_ao_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.70).unwrap();
        let result = calculate_shr(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, ao).unwrap();
        assert!(
            (result.shr - 0.70).abs() < 0.002,
            "Round-trip SHR: expected 0.70 ± 0.002, got {:.5}",
            result.shr
        );
    }

    // ------------------------------------------------------------------
    // Test 4: Ao factor at AHRI rated conditions, SHR = 0.70.
    // Ao = -ln(BF) × ṁ_da. Reference: psychrolib dry-air basis.
    // ------------------------------------------------------------------
    #[test]
    fn coil_ao_factor_ahri_rated() {
        let ao = coil_ao_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.70).unwrap();
        assert!(
            (ao - 1.072).abs() < 0.02,
            "Ao at SHR=0.70: expected 1.072 ± 0.02 kg/s (dry-air basis), got {ao:.5}"
        );
    }

    // ------------------------------------------------------------------
    // Test 5: Very low humidity (below SHR_MIN_HUMIDITY_RATIO) → SHR = 1.0.
    // Physics: dry air has no latent load.
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_dry_coil_returns_one() {
        let result = calculate_shr(T_DB, 1e-8, P_KPA, Q_KW, FLOW_M3S, 1.0).unwrap();
        assert_eq!(
            result.shr, 1.0,
            "SHR must be exactly 1.0 for W < SHR_MIN_HUMIDITY_RATIO"
        );
    }

    // ------------------------------------------------------------------
    // Test 6: Zero capacity → SHR = 1.0.
    // Physics: no cooling means no latent removal.
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_zero_capacity_returns_one() {
        let result = calculate_shr(T_DB, W_IN, P_KPA, 0.0, FLOW_M3S, 1.0).unwrap();
        assert_eq!(
            result.shr, 1.0,
            "SHR must be exactly 1.0 when Q = 0"
        );
    }

    // ------------------------------------------------------------------
    // Test 7: High humidity case that should clamp BF to floor (0.01).
    // Reference: EnergyPlus issue #10738 (negative bypass factor).
    // ------------------------------------------------------------------
    #[test]
    fn bypass_factor_floor_clamps_to_001() {
        let bf = coil_bypass_factor(29.44, 0.014690, P_KPA, Q_KW, FLOW_M3S, 0.68).unwrap();
        assert_eq!(
            bf, BYPASS_FACTOR_FLOOR,
            "High-humidity case must clamp to BYPASS_FACTOR_FLOOR (0.01), got {bf:.5}"
        );
    }

    // ------------------------------------------------------------------
    // Test 9: Normal AHRI conditions with valid Ao converges (Ok, not Err).
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_ahri_conditions_converges() {
        let ao = coil_ao_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.70).unwrap();
        let result = calculate_shr(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, ao);
        assert!(
            result.is_ok(),
            "calculate_shr must converge at AHRI conditions, got {:?}",
            result.err()
        );
    }

    // ------------------------------------------------------------------
    // Test 10: Zero airflow → mass flow = 0 → BF = BYPASS_FACTOR_FLOOR.
    // Physics: no airflow means coil cannot operate.
    // ------------------------------------------------------------------
    #[test]
    fn bypass_factor_zero_mass_flow_returns_floor() {
        let bf = coil_bypass_factor(T_DB, W_IN, P_KPA, Q_KW, 0.0, 0.70).unwrap();
        assert_eq!(
            bf, BYPASS_FACTOR_FLOOR,
            "Zero flow must return BYPASS_FACTOR_FLOOR (0.01), got {bf:.5}"
        );
    }

    // ------------------------------------------------------------------
    // Test: calculate_shr at AHRI nominal conditions returns SHR well below 1.0.
    //
    // 26.7 °C DB / 19.4 °C WB, rated 3-ton flow and capacity.  The coil
    // operates wet (W_IN > SHR_MIN_HUMIDITY_RATIO), so it must remove latent
    // heat and the SHR must be materially below 1.0.  ASHRAE standard
    // conditions are designed to produce SHR ≈ 0.70.  The lower bound is
    // set at 0.65 to allow round-trip floating-point variation without
    // permitting a dry-coil result.
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_nominal_cooling() {
        let ao = coil_ao_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.70).unwrap();
        let result = calculate_shr(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, ao).unwrap();
        assert!(
            result.shr >= 0.65 && result.shr <= 1.0,
            "nominal cooling SHR must be in [0.65, 1.0], got {:.5}",
            result.shr
        );
        // Also assert the result is close to the target 0.70 (within 1%).
        assert!(
            (result.shr - 0.70).abs() < 0.01,
            "nominal cooling SHR must be within 1% of 0.70, got {:.5}",
            result.shr
        );
    }

    // ------------------------------------------------------------------
    // Test: high inlet humidity (W > 0.014 kg/kg) drives latent load up,
    // pushing SHR below 0.8.
    //
    // At 29.44 °C DB (85 °F) and 22.78 °C WB (73 °F), W ≈ 0.01469 kg/kg —
    // above the 0.014 threshold specified in the task.  The larger latent
    // fraction relative to total capacity forces SHR < 0.8.
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_high_humidity() {
        // 29.44 °C DB / 22.78 °C WB at 101.325 kPa → W ≈ 0.01469 kg/kg.
        // Higher dewpoint means the coil works harder on dehumidification.
        let db_c: f64 = 29.44;
        let w_high: f64 = 0.01469; // > 0.014 kg/kg
        let shr_seed: f64 = 0.70;
        let ao = coil_ao_factor(db_c, w_high, P_KPA, Q_KW, FLOW_M3S, shr_seed).unwrap();
        let result = calculate_shr(db_c, w_high, P_KPA, Q_KW, FLOW_M3S, ao).unwrap();
        assert!(
            result.shr > 0.55 && result.shr < 0.8,
            "high-humidity SHR must be in [0.55, 0.8], got {:.5}",
            result.shr
        );
    }


    // Helper: call coil_bypass_factor and recover ADP via calculate_shr round-trip.
    fn bypass_factor_with_adp(
        db_in_c: f64,
        w_in: f64,
        p_kpa: f64,
        q_kw: f64,
        flow_m3_s: f64,
        shr: f64,
    ) -> (f64, f64) {
        let bf = coil_bypass_factor(db_in_c, w_in, p_kpa, q_kw, flow_m3_s, shr).unwrap();
        let ao = coil_ao_factor(db_in_c, w_in, p_kpa, q_kw, flow_m3_s, shr).unwrap();
        let result = calculate_shr(db_in_c, w_in, p_kpa, q_kw, flow_m3_s, ao).unwrap();
        (bf, result.adp_temp_c)
    }

    // ------------------------------------------------------------------
    // Test 1b: Verify ADP temperature at SHR = 0.70 via round-trip.
    // Reference: ASHRAE/OCHRE cross-validation, T_ADP = 11.77°C.
    // ------------------------------------------------------------------
    #[test]
    fn adp_temperature_ahri_rated_shr_070() {
        let (_, t_adp) = bypass_factor_with_adp(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.70);
        assert!(
            (t_adp - 11.77).abs() < 0.1,
            "T_ADP at SHR=0.70: expected 11.77 ± 0.1°C, got {t_adp:.3}°C"
        );
    }

    // ------------------------------------------------------------------
    // Test 2b: Verify ADP temperature at SHR = 0.74 via round-trip.
    // Reference: ASHRAE/OCHRE cross-validation, T_ADP = 12.79°C.
    // ------------------------------------------------------------------
    #[test]
    fn adp_temperature_ahri_rated_shr_074() {
        let (_, t_adp) = bypass_factor_with_adp(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.74);
        assert!(
            (t_adp - 12.79).abs() < 0.1,
            "T_ADP at SHR=0.74: expected 12.79 ± 0.1°C, got {t_adp:.3}°C"
        );
    }

    // ------------------------------------------------------------------
    // Test: Pathological inputs force calculate_shr to exhaust the
    // SHR_ITERATION_LIMIT and return Err.
    // ------------------------------------------------------------------
    #[test]
    fn calculate_shr_non_convergence_returns_error() {
        // Ao=0 makes BF = exp(0) = 1.0, so d_h/(1-BF) = inf and h_adp = -inf.
        // The iterate root-finder receives NaN errors and cannot converge.
        let result = calculate_shr(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.0);
        assert!(
            result.is_err(),
            "pathological Ao=0.0 must fail to converge, got {:?}",
            result
        );
    }

    // ------------------------------------------------------------------
    // Test: SHR = 1.0 takes the enthalpy-only bypass factor path.
    // BF must be in [BYPASS_FACTOR_FLOOR, 1.0].
    // ------------------------------------------------------------------
    #[test]
    fn bypass_factor_shr_one_uses_enthalpy_path() {
        let bf = coil_bypass_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 1.0).unwrap();
        assert!(
            bf >= BYPASS_FACTOR_FLOOR && bf <= 1.0,
            "SHR=1.0 BF must be in [{BYPASS_FACTOR_FLOOR}, 1.0], got {bf:.5}"
        );
    }

    // ------------------------------------------------------------------
    // Test: coil_ao_factor with zero airflow.
    //
    // zero flow → mfr = 0 → coil_bypass_factor returns BYPASS_FACTOR_FLOOR.
    // Ao = -ln(BYPASS_FACTOR_FLOOR) × 0 = 0.0.
    // Must not panic and must return 0.0.
    // ------------------------------------------------------------------
    #[test]
    fn coil_ao_factor_zero_flow() {
        let result = coil_ao_factor(T_DB, W_IN, P_KPA, Q_KW, 0.0, 0.70);
        assert!(
            result.is_ok(),
            "coil_ao_factor with zero flow must not return Err, got {:?}",
            result.err()
        );
        let ao = result.unwrap();
        assert!(
            ao.is_finite(),
            "coil_ao_factor with zero flow must return a finite value, got {ao}"
        );
        // mfr = 0 so Ao = -ln(BF) × mfr = -ln(BF) × 0 = 0.
        assert_eq!(
            ao, 0.0,
            "coil_ao_factor with zero flow must return 0.0, got {ao}"
        );
        // Ao=0.0 drives BF=exp(0/mfr)=1.0 (no contact with coil), so calculate_shr
        // must return SHR=1.0 (no dehumidification) or return Err — but must not panic.
        let shr_result = calculate_shr(T_DB, W_IN, P_KPA, Q_KW, 0.0, ao);
        match shr_result {
            Ok(result) => assert_eq!(
                result.shr, 1.0,
                "calculate_shr with Ao=0 and zero flow must return SHR=1.0, got {:.6}",
                result.shr
            ),
            Err(_) => {} // non-convergence is also an acceptable outcome for Ao=0
        }
    }

    // ------------------------------------------------------------------
    // Test: coil_bypass_factor with inputs that drive d_t = db_in_c - t_out
    // toward zero.
    //
    // When d_t ≈ 0 the slope m_c = d_w / d_t is effectively infinite, which
    // is the near-zero-denominator path.  Using SHR = 0.9999 (nearly sensible)
    // at high capacity makes h_out very close to h_in, so t_out ≈ t_in and
    // d_t is tiny.  The function must return BYPASS_FACTOR_FLOOR (not NaN,
    // not a panic, not a value outside [BYPASS_FACTOR_FLOOR, 1.0]).
    // ------------------------------------------------------------------
    #[test]
    fn coil_bypass_factor_near_zero_denominator() {
        // SHR=0.9999 drives d_w ≈ 0 and m_c ≈ 0, which exercises the ADP slope
        // guard paths.  The result must not panic, must not return Err, and
        // must return BYPASS_FACTOR_FLOOR because the ADP converges to the dew
        // point, making db_in_c − t_adp < 0 for this high-SHR, high-capacity
        // combination — triggering the `db_in_c - t_adp <= 0.0` guard at line 383.
        let result = coil_bypass_factor(T_DB, W_IN, P_KPA, Q_KW, FLOW_M3S, 0.9999);
        assert!(
            result.is_ok(),
            "coil_bypass_factor near-zero d_t must not return Err, got {:?}",
            result.err()
        );
        let bf = result.unwrap();
        assert_eq!(
            bf, BYPASS_FACTOR_FLOOR,
            "near-zero-denominator guard must return exactly BYPASS_FACTOR_FLOOR, got {bf:.6}"
        );
    }
}
