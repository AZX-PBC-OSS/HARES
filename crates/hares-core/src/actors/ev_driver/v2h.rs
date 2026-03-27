//! V2HDischarge — discharge to home when deficit exists.

use super::preference::{
    ChargingPreference, Constraint, DecisionContext, PreferenceVote,
};

pub struct V2HDischarge {
    pub threshold_soc: f64,
    pub min_soc: f64,
    pub max_discharge_kw: f64,
}

impl ChargingPreference for V2HDischarge {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        if ctx.current_soc <= self.min_soc {
            Constraint::Override(PreferenceVote::idle("v2h:soc_floor"))
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let pv = ctx.env.electrical.pv_generation_kw;
        let load = ctx.env.electrical.base_load_kw;
        let deficit = load - pv;

        if ctx.current_soc > self.threshold_soc && deficit > 0.0 {
            let discharge = deficit.min(self.max_discharge_kw);
            PreferenceVote {
                target_soc: None,
                power_kw: Some(-discharge),
                departure_hour: None,
                min_soc: Some(self.min_soc),
                max_soc: None,
                score: 2.0,
                label: "v2h:discharging",
            }
        } else {
            PreferenceVote::idle("v2h:idle")
        }
    }

    fn name(&self) -> &'static str {
        "V2HDischarge"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;
    use hares_types::ElectricalSummary;

    fn make_ctx(
        env: &hares_types::EnvironmentState,
        soc: f64,
    ) -> DecisionContext<'_> {
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
    fn overrides_idle_at_soc_floor() {
        let env = TestEnvBuilder::new().build();
        let mut pref = V2HDischarge {
            threshold_soc: 0.5,
            min_soc: 0.2,
            max_discharge_kw: 5.0,
        };
        let ctx = make_ctx(&env, 0.15);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "v2h:soc_floor");
                assert!(vote.power_kw.is_none());
            }
            Constraint::Inactive => panic!("expected Override"),
        }
    }

    #[test]
    fn discharges_when_deficit_and_soc_above_threshold() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        let mut pref = V2HDischarge {
            threshold_soc: 0.5,
            min_soc: 0.2,
            max_discharge_kw: 5.0,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2h:discharging");
        assert!((vote.power_kw.unwrap() - (-3.0)).abs() < 1e-9);
        assert_eq!(vote.min_soc, Some(0.2));
    }

    #[test]
    fn idles_when_no_deficit() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        let mut pref = V2HDischarge {
            threshold_soc: 0.5,
            min_soc: 0.2,
            max_discharge_kw: 5.0,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2h:idle");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn idles_when_soc_below_threshold() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        let mut pref = V2HDischarge {
            threshold_soc: 0.5,
            min_soc: 0.2,
            max_discharge_kw: 5.0,
        };
        let ctx = make_ctx(&env, 0.4);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2h:idle");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn idles_when_soc_exactly_at_threshold() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 1.0,
                base_load_kw: 4.0,
                ..Default::default()
            })
            .build();
        let mut pref = V2HDischarge {
            threshold_soc: 0.5,
            min_soc: 0.2,
            max_discharge_kw: 5.0,
        };
        // SOC exactly at threshold — strict > means it should idle
        let ctx = make_ctx(&env, 0.5);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2h:idle");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn clamps_discharge_to_max() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                base_load_kw: 10.0,
                ..Default::default()
            })
            .build();
        let mut pref = V2HDischarge {
            threshold_soc: 0.5,
            min_soc: 0.2,
            max_discharge_kw: 5.0,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2h:discharging");
        assert!((vote.power_kw.unwrap() - (-5.0)).abs() < 1e-9);
    }
}
