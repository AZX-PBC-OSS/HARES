//! SolarTracking — modulate charge rate to PV surplus.

use super::preference::{ChargingPreference, DecisionContext, PreferenceVote};

pub struct SolarTracking {
    pub min_charge_rate_kw: f64,
}

impl ChargingPreference for SolarTracking {
    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let pv = ctx.env.electrical.pv_generation_kw;
        let load = ctx.env.electrical.base_load_kw;
        let surplus = (pv - load).max(0.0);

        if surplus < self.min_charge_rate_kw {
            PreferenceVote::idle("solar:insufficient")
        } else {
            let clamped = surplus.min(ctx.max_charge_kw);
            PreferenceVote {
                target_soc: None,
                power_kw: Some(clamped),
                departure_hour: None,
                min_soc: None,
                max_soc: None,
                score: 1.5,
                label: "solar:surplus",
            }
        }
    }

    fn name(&self) -> &'static str {
        "SolarTracking"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;
    use hares_types::ElectricalSummary;

    fn make_ctx(env: &hares_types::EnvironmentState) -> DecisionContext<'_> {
        DecisionContext {
            current_soc: 0.5,
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
    fn charges_at_surplus() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 5.0,
                base_load_kw: 1.5,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_kw: 1.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:surplus");
        assert!((vote.power_kw.unwrap() - 3.5).abs() < 1e-9);
    }

    #[test]
    fn idles_when_insufficient_surplus() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 2.0,
                base_load_kw: 1.5,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_kw: 1.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:insufficient");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn clamps_to_max_charge() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 15.0,
                base_load_kw: 1.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_kw: 1.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:surplus");
        assert!((vote.power_kw.unwrap() - 7.2).abs() < 1e-9);
    }

    #[test]
    fn idles_when_no_pv() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_kw: 0.0,
                base_load_kw: 2.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_kw: 1.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:insufficient");
    }
}
