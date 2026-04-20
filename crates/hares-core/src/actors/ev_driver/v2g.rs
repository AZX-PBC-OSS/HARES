//! V2GExport -- export to grid when price exceeds threshold.

use super::preference::{ChargingPreference, Constraint, DecisionContext, PreferenceVote};

pub struct V2GExport {
    pub min_soc: f64,
    pub max_export_kw: f64,
    pub price_threshold: f64,
}

impl ChargingPreference for V2GExport {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        if ctx.current_soc <= self.min_soc {
            Constraint::Override(PreferenceVote::idle("v2g:soc_floor"))
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let price = ctx.env.price_signal.electricity_price.unwrap_or(0.0);

        if price > self.price_threshold {
            PreferenceVote {
                target_soc: None,
                power_kw: Some(-self.max_export_kw),
                departure_hour: None,
                min_soc: Some(self.min_soc),
                max_soc: None,
                score: 3.0,
                label: "v2g:exporting",
            }
        } else {
            PreferenceVote::idle("v2g:idle")
        }
    }

    fn name(&self) -> &'static str {
        "V2GExport"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;
    use hares_types::PriceSignal;

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
    fn overrides_idle_at_soc_floor() {
        let env = TestEnvBuilder::new().build();
        let mut pref = V2GExport {
            min_soc: 0.3,
            max_export_kw: 5.0,
            price_threshold: 0.20,
        };
        let ctx = make_ctx(&env, 0.25);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "v2g:soc_floor");
                assert!(vote.power_kw.is_none());
            }
            Constraint::Inactive => panic!("expected Override"),
        }
    }

    #[test]
    fn exports_when_price_above_threshold() {
        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.30),
                ..Default::default()
            })
            .build();
        let mut pref = V2GExport {
            min_soc: 0.3,
            max_export_kw: 5.0,
            price_threshold: 0.20,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2g:exporting");
        assert!((vote.power_kw.unwrap() - (-5.0)).abs() < 1e-9);
        assert_eq!(vote.min_soc, Some(0.3));
    }

    #[test]
    fn idles_when_price_below_threshold() {
        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.10),
                ..Default::default()
            })
            .build();
        let mut pref = V2GExport {
            min_soc: 0.3,
            max_export_kw: 5.0,
            price_threshold: 0.20,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2g:idle");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn idles_when_price_exactly_at_threshold() {
        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.20), // exactly at threshold
                ..Default::default()
            })
            .build();
        let mut pref = V2GExport {
            min_soc: 0.3,
            max_export_kw: 5.0,
            price_threshold: 0.20,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        // price > threshold is strict, so exactly-at should idle
        assert_eq!(vote.label, "v2g:idle");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn idles_when_no_price() {
        let env = TestEnvBuilder::new().build();
        let mut pref = V2GExport {
            min_soc: 0.3,
            max_export_kw: 5.0,
            price_threshold: 0.20,
        };
        let ctx = make_ctx(&env, 0.7);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "v2g:idle");
    }
}
