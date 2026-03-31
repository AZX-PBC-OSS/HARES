//! SocGate preference — "only act when SOC below threshold".

use super::preference::{ChargingPreference, Constraint, DecisionContext, PreferenceVote};

pub struct SocGate {
    pub threshold: f64,
    pub target_soc: f64,
}

impl ChargingPreference for SocGate {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        if ctx.current_soc >= self.threshold {
            Constraint::Override(PreferenceVote::idle("soc_gate:above_threshold"))
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        if ctx.current_soc < self.threshold {
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
    fn overrides_idle_when_above_threshold() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.85);
        let mut pref = SocGate {
            threshold: 0.8,
            target_soc: 1.0,
        };

        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert!(vote.power_kw.is_none());
                assert_eq!(vote.label, "soc_gate:above_threshold");
            }
            Constraint::Inactive => panic!("expected Override"),
        }
    }

    #[test]
    fn inactive_when_below_threshold() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.3);
        let mut pref = SocGate {
            threshold: 0.8,
            target_soc: 1.0,
        };

        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
    }

    #[test]
    fn score_at_threshold_returns_idle() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.8);
        let mut pref = SocGate {
            threshold: 0.8,
            target_soc: 1.0,
        };

        let vote = pref.score(&ctx);
        assert_eq!(vote.label, "soc_gate:idle");
        assert!(vote.score.abs() < 1e-9);
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn scores_when_below_threshold() {
        let env = TestEnvBuilder::new().build();
        let ctx = make_ctx(&env, 0.3);
        let mut pref = SocGate {
            threshold: 0.8,
            target_soc: 0.8,
        };

        let vote = pref.score(&ctx);
        assert!((vote.score - 0.5).abs() < 1e-9);
        assert_eq!(vote.target_soc, Some(0.8));
    }
}
