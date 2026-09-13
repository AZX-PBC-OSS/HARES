//! SocGate preference -- "only act when SOC below threshold" with hysteresis.

use super::preference::{
    ChargingPreference, Constraint, DecisionContext, PreferenceVote, needed_charge_hours_to_target,
};

/// Gating preference that prevents rapid charge/no-charge cycling near a single
/// threshold by using separate upper and lower bounds with hysteresis.
///
/// - When `charging_allowed` is `true`, charging continues until SOC reaches or
///   exceeds `upper_threshold`, at which point it is blocked.
/// - When `charging_allowed` is `false`, charging remains blocked until SOC
///   drops below `lower_threshold`, at which point it is re-allowed.
pub struct SocGate {
    pub upper_threshold: f64,
    pub lower_threshold: f64,
    pub target_soc: f64,
    /// Base charging efficiency (dimensionless, e.g. 0.9) used for the
    /// time-to-charge estimate. Temperature degradation is applied on top
    /// inside [`needed_charge_hours_to_target`].
    pub charging_efficiency: f64,
    /// Internal hysteresis state: `true` = charging currently allowed.
    pub charging_allowed: bool,
}

impl Default for SocGate {
    fn default() -> Self {
        Self {
            upper_threshold: 1.0,
            lower_threshold: 0.95,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: true,
        }
    }
}

impl ChargingPreference for SocGate {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        if self.charging_allowed && ctx.current_soc >= self.upper_threshold {
            self.charging_allowed = false;
            Constraint::Override(PreferenceVote::idle("soc_gate:above_upper_threshold"))
        } else if !self.charging_allowed && ctx.current_soc < self.lower_threshold {
            self.charging_allowed = true;
            Constraint::Inactive
        } else if !self.charging_allowed {
            Constraint::Override(PreferenceVote::idle("soc_gate:hysteresis_hold"))
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        if self.charging_allowed && ctx.current_soc < self.upper_threshold {
            let gap = (self.target_soc - ctx.current_soc).max(0.0);
            PreferenceVote {
                target_soc: Some(self.target_soc),
                power_kw: None,
                departure_hour: None,
                min_soc: None,
                max_soc: None,
                score: gap,
                label: "soc_gate:charging",
            }
        } else {
            PreferenceVote::idle("soc_gate:idle")
        }
    }

    fn name(&self) -> &'static str {
        "SocGate"
    }

    fn charging_allowed(&self) -> Option<bool> {
        Some(self.charging_allowed)
    }

    /// Time to charge from the current SOC to the gate's target, reported
    /// regardless of the hysteresis state: the estimate is a property of the
    /// battery gap (what the channel name promises), not the strategy's
    /// intent — a resting `QuickThenWait`/`LowSoc` vehicle still needs those
    /// hours to close the gap.
    fn needed_charge_hours(&self, ctx: &DecisionContext) -> f64 {
        needed_charge_hours_to_target(self.target_soc, self.charging_efficiency, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;

    fn make_ctx(env: &hares_types::EnvironmentState, soc: f64) -> DecisionContext<'_> {
        DecisionContext {
            current_soc: soc,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env,
            current_minute: 720,
            next_departure_minute: None,
            time_res_minutes: 1.0,
        }
    }

    #[test]
    fn overrides_idle_when_above_upper_threshold() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.85);
        let mut pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert!(vote.power_kw.is_none());
                assert_eq!(vote.label, "soc_gate:above_upper_threshold");
            }
            Constraint::Inactive => panic!("expected Override"),
        }
        assert!(!pref.charging_allowed);
    }

    #[test]
    fn inactive_when_below_lower_allowed() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.3);
        let mut pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
    }

    #[test]
    fn score_at_upper_threshold_returns_idle() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.8);
        let mut pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        let vote = pref.score(&ctx);
        assert_eq!(vote.label, "soc_gate:idle");
        assert!(vote.score.abs() < 1e-9);
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn scores_when_below_upper_threshold() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.3);
        let mut pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 0.8,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        let vote = pref.score(&ctx);
        assert!((vote.score - 0.5).abs() < 1e-9);
        assert_eq!(vote.target_soc, Some(0.8));
    }

    // ---- Hysteresis behavior tests ----

    #[test]
    fn soc_gate_stops_at_upper_resumes_at_lower() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        // SOC 0.6 — well below lower, charging allowed
        let ctx = make_ctx(&env, 0.6);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        assert!(pref.charging_allowed);

        // SOC reaches 0.8 — upper threshold hit, charging stops
        let ctx = make_ctx(&env, 0.8);
        match pref.constraint(&ctx) {
            Constraint::Override(_) => {}
            _ => panic!("expected Override at upper_threshold"),
        }
        assert!(!pref.charging_allowed);

        // SOC 0.75 — between thresholds, charging must NOT restart
        let ctx = make_ctx(&env, 0.75);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "soc_gate:hysteresis_hold");
            }
            _ => panic!("expected Override(hysteresis_hold) between thresholds"),
        }
        assert!(!pref.charging_allowed);

        // SOC 0.69 — drops below lower_threshold, charging resumes
        let ctx = make_ctx(&env, 0.69);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        assert!(pref.charging_allowed);

        // SOC 0.75 — still below upper, charging continues
        let ctx = make_ctx(&env, 0.75);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        assert!(pref.charging_allowed);
    }

    #[test]
    fn soc_gate_quick_then_wait_rests() {
        let env = TestEnvBuilder::new().build();
        let partial_soc = 0.6;
        let band = 0.05;
        let mut pref = SocGate {
            upper_threshold: partial_soc,
            lower_threshold: partial_soc - band,
            target_soc: partial_soc,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        // Below lower — charging allowed
        let ctx = make_ctx(&env, 0.4);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        assert!(pref.charging_allowed);

        // Reach partial_soc — charging stops
        let ctx = make_ctx(&env, partial_soc);
        match pref.constraint(&ctx) {
            Constraint::Override(_) => {}
            _ => panic!("expected Override at partial_soc"),
        }
        assert!(!pref.charging_allowed);

        // Slight self-discharge to 0.58 — still above lower (0.55), stays idle
        let ctx = make_ctx(&env, 0.58);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "soc_gate:hysteresis_hold");
            }
            _ => panic!("expected hysteresis hold at 0.58 (above lower=0.55)"),
        }
        assert!(!pref.charging_allowed);

        // Drop below lower (0.55) — charging resumes
        let ctx = make_ctx(&env, 0.54);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        assert!(pref.charging_allowed);
    }

    #[test]
    fn soc_gate_score_idles_when_not_allowed() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: false,
        };

        // Even though SOC is below upper_threshold, charging_allowed=false means idle
        let ctx = make_ctx(&env, 0.5);
        let vote = pref.score(&ctx);
        assert_eq!(vote.label, "soc_gate:idle");
        assert!(vote.score.abs() < 1e-9);
    }

    #[test]
    fn soc_gate_charging_allowed_visible() {
        let pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 1.0,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };
        assert_eq!(pref.charging_allowed(), Some(true));
    }

    #[test]
    fn needed_charge_hours_reports_gap_even_while_hysteresis_hold() {
        // A resting gate (charging_allowed=false between thresholds) still
        // reports the physical time to close the gap — the estimate is a
        // property of the battery, not the strategy's intent.
        let env = TestEnvBuilder::new().outdoor_temp(22.0).build();
        let pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 0.8,
            charging_efficiency: 0.9,
            charging_allowed: false,
        };

        // SOC 0.75, target 0.8: gap 0.05 on 60 kWh = 3 kWh;
        // 3 / (7.2 * 0.9) h, rounded up to whole 1-minute steps.
        let ctx = make_ctx(&env, 0.75);
        let raw: f64 = 3.0 / (7.2 * 0.9);
        let step_hours: f64 = 1.0 / 60.0;
        let expected = (raw / step_hours).ceil() * step_hours;
        let hours = pref.needed_charge_hours(&ctx);
        assert!(
            (hours - expected).abs() < 1e-9,
            "needed_charge_hours while resting ({hours}) should equal the constant-efficiency expected ({expected}) at 22C"
        );
    }

    #[test]
    fn needed_charge_hours_zero_at_target() {
        let env = TestEnvBuilder::new().outdoor_temp(22.0).build();
        let pref = SocGate {
            upper_threshold: 0.8,
            lower_threshold: 0.7,
            target_soc: 0.8,
            charging_efficiency: 0.9,
            charging_allowed: true,
        };

        let ctx = make_ctx(&env, 0.8);
        assert!(
            pref.needed_charge_hours(&ctx).abs() < 1e-9,
            "needed_charge_hours must be 0.0 when the SOC gap is zero"
        );
    }
}
