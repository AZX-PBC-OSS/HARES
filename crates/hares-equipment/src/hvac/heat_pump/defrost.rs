//! Defrost control for heat-pump heating: OnDemand (humidity-based) and Timed modes.

use hares_physics::psychrometrics::humidity_ratio_from_twb;
use serde::{Deserialize, Serialize};

use super::constants::{
    DEFAULT_DEFROST_TIME_FRACTION, DEFROST_CAPACITY_MULTIPLIER_BASE, DEFROST_CAPACITY_UNIT_FACTOR,
    DEFROST_COIL_TEMP_OFFSET_C, DEFROST_COIL_TEMP_SLOPE, DEFROST_EIR_CURVE_TEMP_MIN_C,
    DEFROST_EIR_TEMP_MODIFIER, DEFROST_ENABLE_TEMP_C, DEFROST_MIN_DELTA_HUMIDITY_RATIO,
    DEFROST_POWER_MULTIPLIER_NUMERATOR, DEFROST_Q_MULTIPLIER, DEFROST_REFERENCE_TEMP_C,
    DEFROST_TIME_FRACTION_NUMERATOR, TIMED_DEFROST_CAP_MULT_BASE, TIMED_DEFROST_CAP_MULT_SLOPE,
    TIMED_DEFROST_PWR_MULT_BASE, TIMED_DEFROST_PWR_MULT_SLOPE,
};

/// Defrost activation / timing strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefrostControl {
    /// Humidity-based — defrost fraction computed from outdoor coil moisture
    /// accumulation. More physical but requires humidity data (OCHRE default).
    OnDemand,
    /// Timer-based — fixed defrost time fraction. Simpler, used by many real
    /// units. Uses different capacity/EIR multiplier equations from DOE-2.
    Timed,
}

/// How defrost heat is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefrostStrategy {
    /// Reverse refrigerant cycle (most common in modern heat pumps).
    ReverseCycle,
    /// Resistive heating element.
    Resistive,
}

/// Full defrost configuration including control mode and strategy.
#[derive(Clone, Copy, Debug)]
pub struct DefrostConfig {
    /// Legacy capacity scaling factor applied after defrost multiplier [0..1].
    pub capacity_reduction_factor: f64,
    /// Additional fixed defrost power draw [W] (legacy/OCHRE field).
    pub defrost_power_w: f64,
    /// Control mode: OnDemand (humidity-based) or Timed.
    pub control: DefrostControl,
    /// Defrost strategy: ReverseCycle or Resistive.
    pub strategy: DefrostStrategy,
    /// For Timed mode: fraction of hour spent in defrost [0..1].
    /// Typical value: 0.058 (~3.5 min/hr). Ignored in OnDemand mode.
    pub defrost_time_fraction: f64,
    /// Maximum OAT for defrost activation [°C].
    pub max_oat_defrost_c: f64,
    /// For ReverseCycle strategy: optional biquadratic EIR curve coefficients
    /// `[c0, c1, c2, c3, c4, c5]` evaluated at `(wb, db)` with a 15.555°C floor.
    /// `None` means no EIR adjustment (factor = 1.0).
    pub defrost_eir_coeffs: Option<[f64; 6]>,
    /// For Resistive strategy: rated defrost heater capacity [W].
    pub resistive_defrost_capacity_w: f64,
}

impl DefrostConfig {
    /// Construct a backward-compatible OnDemand config from legacy fields.
    #[must_use]
    pub fn on_demand(capacity_reduction_factor: f64, defrost_power_w: f64) -> Self {
        Self {
            capacity_reduction_factor,
            defrost_power_w,
            control: DefrostControl::OnDemand,
            strategy: DefrostStrategy::ReverseCycle,
            defrost_time_fraction: DEFAULT_DEFROST_TIME_FRACTION,
            max_oat_defrost_c: DEFROST_ENABLE_TEMP_C,
            defrost_eir_coeffs: None,
            resistive_defrost_capacity_w: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefrostResult {
    pub active: bool,
    pub time_fraction: f64,
    pub capacity_multiplier: f64,
    pub power_multiplier: f64,
    pub q_defrost_w: f64,
    pub extra_power_w: f64,
}

impl DefrostResult {
    fn inactive() -> Self {
        Self {
            active: false,
            capacity_multiplier: 1.0,
            power_multiplier: 1.0,
            ..Self::default()
        }
    }
}

/// Evaluate defrost adjustments for the current timestep.
///
/// # Parameters
/// - `config` — defrost configuration (control mode, strategy, etc.)
/// - `outdoor_db_c` — outdoor dry-bulb temperature [°C]
/// - `outdoor_humidity_ratio` — outdoor humidity ratio [kg/kg]
/// - `pressure_pa` — outdoor air pressure [Pa]
/// - `inlet_wb_c` — indoor inlet wet-bulb temperature [°C]; used for the
///   optional biquadratic EIR curve in Timed + ReverseCycle mode
/// - `rated_capacity_w` — heat pump rated (maximum) heating capacity [W]
/// - `current_capacity_w` — current operating heating capacity [W]
/// - `runtime_fraction` — compressor runtime fraction [0..1]; scales timed
///   defrost power proportionally with compressor operation
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn evaluate_defrost(
    config: &DefrostConfig,
    outdoor_db_c: f64,
    outdoor_humidity_ratio: f64,
    pressure_pa: f64,
    inlet_wb_c: f64,
    rated_capacity_w: f64,
    current_capacity_w: f64,
    runtime_fraction: f64,
) -> DefrostResult {
    if outdoor_db_c >= config.max_oat_defrost_c
        || current_capacity_w <= 0.0
        || rated_capacity_w <= 0.0
    {
        return DefrostResult::inactive();
    }

    // Shared coil temperature and delta-humidity computation used by both modes.
    let coil_out_temp_c = DEFROST_COIL_TEMP_SLOPE * outdoor_db_c + DEFROST_COIL_TEMP_OFFSET_C;
    let omega_sat_coil = humidity_ratio_from_twb(coil_out_temp_c, coil_out_temp_c, pressure_pa);
    let delta_omega =
        (outdoor_humidity_ratio - omega_sat_coil).max(DEFROST_MIN_DELTA_HUMIDITY_RATIO);

    match config.control {
        DefrostControl::OnDemand => {
            let time_fraction =
                (1.0 / (1.0 + DEFROST_TIME_FRACTION_NUMERATOR / delta_omega)).clamp(0.0, 1.0);
            let capacity_multiplier =
                (DEFROST_CAPACITY_MULTIPLIER_BASE * (1.0 - time_fraction)).clamp(0.0, 1.0);
            // power_multiplier is a fixed ratio (≈1.09) intentionally > 1.0 in the
            // OCHRE-derived OnDemand model; it is not subject to the negative-value
            // defect and must not be clamped.
            let power_multiplier =
                DEFROST_POWER_MULTIPLIER_NUMERATOR / DEFROST_CAPACITY_MULTIPLIER_BASE;

            let q_defrost_w = DEFROST_Q_MULTIPLIER
                * time_fraction
                * (DEFROST_REFERENCE_TEMP_C - outdoor_db_c)
                * (rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR);

            // Use post-defrost capacity for extra power calculation
            // (OCHRE HVAC.py L1162: power_defrost uses capacity after defrost reduction).
            let post_defrost_cap_w =
                (current_capacity_w * capacity_multiplier - q_defrost_w).max(0.0);
            let extra_power_w = DEFROST_EIR_TEMP_MODIFIER
                * (post_defrost_cap_w / DEFROST_CAPACITY_UNIT_FACTOR)
                * time_fraction
                + config.defrost_power_w;

            DefrostResult {
                active: true,
                time_fraction,
                capacity_multiplier,
                power_multiplier,
                q_defrost_w,
                extra_power_w,
            }
        }

        DefrostControl::Timed => {
            let time_fraction = config.defrost_time_fraction;
            if time_fraction <= 0.0 {
                return DefrostResult::inactive();
            }

            // Timed mode multipliers from EnergyPlus / DOE-2.
            // Clamped to [0.0, 1.0]: high delta_omega can drive the linear equations
            // below zero, which is physically impossible (negative capacity / power).
            let capacity_multiplier = (TIMED_DEFROST_CAP_MULT_BASE
                - TIMED_DEFROST_CAP_MULT_SLOPE * delta_omega)
                .clamp(0.0, 1.0);
            let power_multiplier = (TIMED_DEFROST_PWR_MULT_BASE
                - TIMED_DEFROST_PWR_MULT_SLOPE * delta_omega)
                .clamp(0.0, 1.0);

            let (q_defrost_w, extra_power_w) = match config.strategy {
                DefrostStrategy::ReverseCycle => {
                    let q = 0.01
                        * time_fraction
                        * (DEFROST_REFERENCE_TEMP_C - outdoor_db_c)
                        * (rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR);

                    let defrost_eir = match config.defrost_eir_coeffs {
                        Some(c) => {
                            let wb = inlet_wb_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C);
                            let db = outdoor_db_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C);
                            c[0] + c[1] * wb
                                + c[2] * wb * wb
                                + c[3] * db
                                + c[4] * wb * db
                                + c[5] * db * db
                        }
                        None => 1.0,
                    };

                    let power = defrost_eir
                        * (rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR)
                        * time_fraction
                        * runtime_fraction
                        + config.defrost_power_w * time_fraction;

                    (q, power)
                }
                DefrostStrategy::Resistive => {
                    let power =
                        config.resistive_defrost_capacity_w * time_fraction * runtime_fraction
                            + config.defrost_power_w * time_fraction;
                    (0.0, power)
                }
            };

            DefrostResult {
                active: true,
                time_fraction,
                capacity_multiplier,
                power_multiplier,
                q_defrost_w,
                extra_power_w,
            }
        }
    }
}

#[cfg(test)]
mod defrost_tests {
    use super::*;

    const P_PA: f64 = 101_325.0;

    fn on_demand_config() -> DefrostConfig {
        DefrostConfig::on_demand(1.0, 0.0)
    }

    fn timed_config() -> DefrostConfig {
        DefrostConfig {
            capacity_reduction_factor: 1.0,
            defrost_power_w: 0.0,
            control: DefrostControl::Timed,
            strategy: DefrostStrategy::ReverseCycle,
            defrost_time_fraction: DEFAULT_DEFROST_TIME_FRACTION,
            max_oat_defrost_c: DEFROST_ENABLE_TEMP_C,
            defrost_eir_coeffs: None,
            resistive_defrost_capacity_w: 0.0,
        }
    }

    /// Test 1: OnDemand mode is unchanged from current HARES implementation.
    #[test]
    fn on_demand_regression() {
        let cfg = on_demand_config();
        // Conditions that previously produced an active defrost result.
        let result = evaluate_defrost(&cfg, 0.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active, "OnDemand must be active below threshold");
        assert!(
            result.time_fraction > 0.0 && result.time_fraction < 1.0,
            "time_fraction must be in (0, 1)"
        );
        assert!(
            result.capacity_multiplier > 0.0 && result.capacity_multiplier < 1.0,
            "capacity_multiplier must be in (0, 1)"
        );
        assert!(result.q_defrost_w >= 0.0, "q_defrost must be non-negative");
        assert!(
            result.extra_power_w >= 0.0,
            "extra_power must be non-negative"
        );
        // power_multiplier is constant in OnDemand mode.
        let expected_pwr = DEFROST_POWER_MULTIPLIER_NUMERATOR / DEFROST_CAPACITY_MULTIPLIER_BASE;
        assert!(
            (result.power_multiplier - expected_pwr).abs() < 1e-12,
            "power_multiplier {:.6} != expected {expected_pwr:.6}",
            result.power_multiplier
        );
    }

    /// Test 2: Timed mode produces correct capacity multiplier.
    #[test]
    fn timed_capacity_multiplier_correct() {
        let cfg = timed_config();
        // At OAT = 0°C with a specific humidity ratio, verify the multiplier formula.
        let outdoor_hr = 0.005_f64;
        let result = evaluate_defrost(&cfg, 0.0, outdoor_hr, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);

        // Recompute expected value.
        let coil_t = DEFROST_COIL_TEMP_SLOPE * 0.0 + DEFROST_COIL_TEMP_OFFSET_C;
        let omega_sat = humidity_ratio_from_twb(coil_t, coil_t, P_PA);
        let dw = (outdoor_hr - omega_sat).max(DEFROST_MIN_DELTA_HUMIDITY_RATIO);
        let expected_cap = TIMED_DEFROST_CAP_MULT_BASE - TIMED_DEFROST_CAP_MULT_SLOPE * dw;
        assert!(
            (result.capacity_multiplier - expected_cap).abs() < 1e-12,
            "capacity_multiplier {:.8} != expected {expected_cap:.8}",
            result.capacity_multiplier
        );
    }

    /// Test 3: Timed mode produces correct power multiplier.
    #[test]
    fn timed_power_multiplier_correct() {
        let cfg = timed_config();
        let outdoor_hr = 0.005_f64;
        let result = evaluate_defrost(&cfg, 0.0, outdoor_hr, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);

        let coil_t = DEFROST_COIL_TEMP_SLOPE * 0.0 + DEFROST_COIL_TEMP_OFFSET_C;
        let omega_sat = humidity_ratio_from_twb(coil_t, coil_t, P_PA);
        let dw = (outdoor_hr - omega_sat).max(DEFROST_MIN_DELTA_HUMIDITY_RATIO);
        let expected_pwr = TIMED_DEFROST_PWR_MULT_BASE - TIMED_DEFROST_PWR_MULT_SLOPE * dw;
        assert!(
            (result.power_multiplier - expected_pwr).abs() < 1e-12,
            "power_multiplier {:.8} != expected {expected_pwr:.8}",
            result.power_multiplier
        );
    }

    /// Test 4: Timed and OnDemand yield different results for the same conditions.
    #[test]
    fn timed_vs_on_demand_differ() {
        let od = on_demand_config();
        let td = timed_config();
        let r_od = evaluate_defrost(&od, 0.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        let r_td = evaluate_defrost(&td, 0.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        // The formulas differ — at minimum the capacity multiplier must differ.
        assert!(
            (r_od.capacity_multiplier - r_td.capacity_multiplier).abs() > 1e-6,
            "OnDemand and Timed capacity multipliers should differ"
        );
    }

    /// Test 5: Above max_oat_defrost, neither mode activates.
    #[test]
    fn no_defrost_above_max_oat() {
        for cfg in [on_demand_config(), timed_config()] {
            let result = evaluate_defrost(
                &cfg,
                cfg.max_oat_defrost_c + 1.0,
                0.005,
                P_PA,
                10.0,
                8_000.0,
                6_000.0,
                1.0,
            );
            assert!(
                !result.active,
                "{:?} must be inactive above max_oat_defrost_c",
                cfg.control
            );
            assert_eq!(result.capacity_multiplier, 1.0);
            assert_eq!(result.power_multiplier, 1.0);
        }
    }

    /// Test 6: ReverseCycle strategy — q_defrost and extra_power are positive.
    #[test]
    fn reverse_cycle_q_defrost_and_power() {
        let cfg = timed_config(); // ReverseCycle by default
        let result = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);
        assert!(
            result.q_defrost_w > 0.0,
            "ReverseCycle must produce positive q_defrost"
        );
        assert!(
            result.extra_power_w > 0.0,
            "ReverseCycle must produce positive extra_power"
        );
    }

    /// Test 7: Resistive strategy — q_defrost = 0, power from heater capacity.
    #[test]
    fn resistive_strategy_no_reverse_cycle_load() {
        let cfg = DefrostConfig {
            strategy: DefrostStrategy::Resistive,
            resistive_defrost_capacity_w: 1_000.0,
            ..timed_config()
        };
        let result = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);
        assert_eq!(
            result.q_defrost_w, 0.0,
            "Resistive must have zero q_defrost"
        );
        let expected_power = 1_000.0 * DEFAULT_DEFROST_TIME_FRACTION * 1.0; // rtf=1.0, defrost_power_w=0
        assert!(
            (result.extra_power_w - expected_power).abs() < 1e-9,
            "Resistive extra_power {:.6} != expected {expected_power:.6}",
            result.extra_power_w
        );
    }

    /// Test 8: Defrost EIR curve is evaluated with 15.555°C floor on inputs.
    #[test]
    fn defrost_eir_curve_with_floor_clipping() {
        // Constant-1.0 curve: c0=1, others=0 → EIR always 1.0 regardless of temp
        let constant_one = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let cfg_flat = DefrostConfig {
            defrost_eir_coeffs: Some(constant_one),
            ..timed_config()
        };
        // A non-trivial curve: purely wb-linear, c1=1 → EIR = wb (floored at 15.555)
        let linear_wb = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let cfg_linear = DefrostConfig {
            defrost_eir_coeffs: Some(linear_wb),
            ..timed_config()
        };

        // With inlet_wb = 5°C < 15.555 → clamped to 15.555.
        let r_flat = evaluate_defrost(&cfg_flat, -5.0, 0.005, P_PA, 5.0, 8_000.0, 6_000.0, 1.0);
        let r_linear = evaluate_defrost(&cfg_linear, -5.0, 0.005, P_PA, 5.0, 8_000.0, 6_000.0, 1.0);

        assert!(r_flat.active && r_linear.active);
        // linear curve EIR = floored wb = 15.555; flat EIR = 1.0
        // power_linear / power_flat should be ≈ 15.555
        let ratio = r_linear.extra_power_w / r_flat.extra_power_w;
        assert!(
            (ratio - DEFROST_EIR_CURVE_TEMP_MIN_C).abs() < 1e-6,
            "EIR curve ratio {ratio:.6} != expected {DEFROST_EIR_CURVE_TEMP_MIN_C}"
        );
    }

    /// Test 9: Runtime fraction scales defrost power proportionally.
    #[test]
    fn runtime_fraction_scales_defrost_power() {
        let cfg = timed_config();
        let r_full = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        let r_half = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 0.5);
        assert!(r_full.active && r_half.active);
        let ratio = r_half.extra_power_w / r_full.extra_power_w;
        assert!(
            (ratio - 0.5).abs() < 1e-9,
            "defrost power ratio {ratio:.6} != 0.5 for half runtime fraction"
        );
    }

    /// Test 10: time_fraction = 0 in Timed mode → inactive result.
    #[test]
    fn timed_zero_time_fraction_is_inactive() {
        let cfg = DefrostConfig {
            defrost_time_fraction: 0.0,
            ..timed_config()
        };
        let result = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(!result.active, "time_fraction=0 must yield inactive result");
    }

    /// Test 11: Very cold OAT (-20°C) — both modes produce finite, reasonable values.
    #[test]
    fn very_cold_oat_produces_finite_values() {
        for cfg in [on_demand_config(), timed_config()] {
            let result = evaluate_defrost(&cfg, -20.0, 0.0005, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
            assert!(result.active, "{:?} must be active at -20°C", cfg.control);
            assert!(
                result.capacity_multiplier.is_finite(),
                "capacity_multiplier must be finite at -20°C"
            );
            assert!(
                result.power_multiplier.is_finite(),
                "power_multiplier must be finite at -20°C"
            );
            assert!(
                result.q_defrost_w.is_finite() && result.q_defrost_w >= 0.0,
                "q_defrost_w must be finite and non-negative at -20°C"
            );
            assert!(
                result.extra_power_w.is_finite() && result.extra_power_w >= 0.0,
                "extra_power_w must be finite and non-negative at -20°C"
            );
        }
    }

    /// Test 12: Timed multipliers match EnergyPlus reference values for known conditions.
    ///
    /// At OAT = 0°C, outdoor_hr = 0.003 kg/kg, the coil outlet temp is
    /// `0.82 * 0.0 - 8.589 = -8.589°C`. The saturation HR at -8.589°C is very small
    /// (sub-zero), so delta_omega ≈ 0.003. We verify the formula output is physically
    /// plausible: cap_mult < 1.0, pwr_mult < 1.0.
    #[test]
    fn timed_multipliers_plausible_energyplus_reference() {
        let cfg = timed_config();
        // Use a moderate humidity ratio (0.003) at OAT=0°C.
        let result = evaluate_defrost(&cfg, 0.0, 0.003, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        // EnergyPlus reference: at typical winter conditions, cap_mult < 1.0 and
        // pwr_mult < 1.0 indicate reduced capacity / power during defrost.
        assert!(
            result.capacity_multiplier < 1.0,
            "cap_mult {:.4} must be < 1.0 in defrost",
            result.capacity_multiplier
        );
        assert!(
            result.capacity_multiplier > 0.0,
            "cap_mult must be positive"
        );
        assert!(
            result.power_multiplier < 1.0,
            "pwr_mult {:.4} must be < 1.0 in defrost",
            result.power_multiplier
        );
        assert!(result.power_multiplier > 0.0, "pwr_mult must be positive");
    }

    /// Test 13: Timed multipliers are clamped to [0.0, 1.0] when delta_omega exceeds
    /// the threshold that would drive the linear equations negative.
    ///
    /// TIMED_DEFROST_CAP_MULT_SLOPE = 107.33, so cap_mult goes negative above
    /// delta_omega ≈ 0.909 / 107.33 ≈ 0.00847 kg/kg.
    /// TIMED_DEFROST_PWR_MULT_SLOPE = 36.45, so pwr_mult goes negative above
    /// delta_omega ≈ 0.90 / 36.45 ≈ 0.0247 kg/kg.
    #[test]
    fn timed_multipliers_clamped_at_high_humidity() {
        let cfg = timed_config();
        // delta_omega ≈ 0.01 > 0.00847 — drives cap_mult negative without the clamp.
        let result = evaluate_defrost(&cfg, 0.0, 0.011, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        assert!(
            result.capacity_multiplier >= 0.0,
            "capacity_multiplier must be >= 0.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.capacity_multiplier <= 1.0,
            "capacity_multiplier must be <= 1.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.power_multiplier >= 0.0,
            "power_multiplier must be >= 0.0, got {:.6}",
            result.power_multiplier
        );
        assert!(
            result.power_multiplier <= 1.0,
            "power_multiplier must be <= 1.0, got {:.6}",
            result.power_multiplier
        );
    }

    /// Test 14: Both timed multipliers clamp to exactly 0.0 at extreme humidity,
    /// not to a negative value.
    #[test]
    fn timed_multipliers_clamped_at_extreme_humidity() {
        let cfg = timed_config();
        // delta_omega ≈ 0.05 — well above both thresholds.
        // Without the clamp: cap_mult = 0.909 - 107.33*0.05 ≈ -4.46
        //                    pwr_mult = 0.90  -  36.45*0.05 ≈ -0.92
        let result = evaluate_defrost(&cfg, 0.0, 0.051, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        assert_eq!(
            result.capacity_multiplier, 0.0,
            "capacity_multiplier must be exactly 0.0 at extreme humidity"
        );
        assert_eq!(
            result.power_multiplier, 0.0,
            "power_multiplier must be exactly 0.0 at extreme humidity"
        );
    }

    /// Test 15: OnDemand capacity multiplier stays in [0.0, 1.0] and power
    /// multiplier stays non-negative even at extreme humidity that drives
    /// time_fraction toward 1.0.
    ///
    /// As delta_omega → ∞, time_fraction → 1.0 and capacity_multiplier → 0.0.
    /// The capacity clamp guards against floating-point edge cases going below 0.
    ///
    /// Note: OnDemand power_multiplier is a fixed ratio (≈1.09) derived from the
    /// OCHRE/EnergyPlus formula. It is intentionally > 1.0 and is not subject to
    /// the negative-value defect fixed in Timed mode.
    #[test]
    fn on_demand_multipliers_clamped_when_time_fraction_approaches_one() {
        let cfg = on_demand_config();
        // Very high humidity ratio forces time_fraction very close to 1.0.
        let result = evaluate_defrost(&cfg, 0.0, 0.5, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        assert!(
            result.capacity_multiplier >= 0.0,
            "OnDemand capacity_multiplier must be >= 0.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.capacity_multiplier <= 1.0,
            "OnDemand capacity_multiplier must be <= 1.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.power_multiplier >= 0.0,
            "OnDemand power_multiplier must be non-negative, got {:.6}",
            result.power_multiplier
        );
        // time_fraction must be strictly in (0, 1).
        assert!(
            result.time_fraction > 0.0 && result.time_fraction < 1.0,
            "time_fraction {:.8} must be in (0, 1)",
            result.time_fraction
        );
    }
}
