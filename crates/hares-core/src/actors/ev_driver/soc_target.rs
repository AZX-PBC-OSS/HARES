//! SocTarget preference -- "charge to X%".

use super::preference::{ChargingPreference, DecisionContext, PreferenceVote};

pub struct SocTarget {
    pub target_soc: f64,
}

impl ChargingPreference for SocTarget {
    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let gap = (self.target_soc - ctx.current_soc).max(0.0);
        PreferenceVote {
            target_soc: Some(self.target_soc),
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: gap,
            label: "soc_target",
        }
    }

    fn name(&self) -> &'static str {
        "SocTarget"
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
    fn score_proportional_to_gap() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocTarget { target_soc: 0.9 };

        let ctx_low = make_ctx(&env, 0.3);
        let vote_low = pref.score(&ctx_low);
        assert!((vote_low.score - 0.6).abs() < 1e-9);
        assert_eq!(vote_low.target_soc, Some(0.9));

        let ctx_high = make_ctx(&env, 0.8);
        let vote_high = pref.score(&ctx_high);
        assert!((vote_high.score - 0.1).abs() < 1e-9);
    }

    #[test]
    fn score_zero_when_at_target() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocTarget { target_soc: 0.8 };

        let ctx = make_ctx(&env, 0.8);
        let vote = pref.score(&ctx);
        assert!(vote.score.abs() < 1e-9);
    }

    #[test]
    fn score_zero_when_above_target() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocTarget { target_soc: 0.9 };

        let ctx = make_ctx(&env, 0.95);
        let vote = pref.score(&ctx);
        assert!(vote.score.abs() < 1e-9);
        assert_eq!(vote.target_soc, Some(0.9));
    }
}
