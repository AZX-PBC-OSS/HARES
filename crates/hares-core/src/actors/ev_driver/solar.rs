//! SolarTracking -- modulate charge rate to PV surplus.

use super::preference::{ChargingPreference, DecisionContext, PreferenceVote};

pub struct SolarTracking {
    pub min_charge_rate_w: f64,
}

impl ChargingPreference for SolarTracking {
    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        // The electrical summary is W (SI), read with no conversion.

        // The observation's own build site checks finiteness, and a negative
        // PV observation fails the step there; no per-decide re-check.

        let pv = ctx.env.electrical.actual_pv_w_or_fallback();
        let load = ctx.env.electrical.base_load_w;
        let surplus = (pv - load).max(0.0);

        if surplus < self.min_charge_rate_w {
            PreferenceVote::idle("solar:insufficient")
        } else {
            let clamped = surplus.min(ctx.max_charge_w);
            PreferenceVote {
                target_soc: None,
                power_w: Some(clamped),
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
            capacity_j: 216_000_000.0, // 60 kWh
            max_charge_w: 7200.0,
            max_discharge_w: 5000.0,
            env,
            current_minute: 720,
            next_departure_minute: None,
            time_res_minutes: 1.0,
            observed_charge_derate: None,
        }
    }

    #[test]
    fn charges_at_surplus() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_w: 5000.0,
                base_load_w: 1500.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_w: 1000.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:surplus");
        assert!((vote.power_w.unwrap() - 3500.0).abs() < 1e-9);
    }

    #[test]
    fn idles_when_insufficient_surplus() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_w: 2000.0,
                base_load_w: 1500.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_w: 1000.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:insufficient");
        assert!(vote.power_w.is_none());
    }

    #[test]
    fn clamps_to_max_charge() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_w: 15000.0,
                base_load_w: 1000.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_w: 1000.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:surplus");
        assert!((vote.power_w.unwrap() - 7200.0).abs() < 1e-9);
    }

    #[test]
    fn idles_when_no_pv() {
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_w: 0.0,
                base_load_w: 2000.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_w: 1000.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:insufficient");
        assert!(vote.power_w.is_none());
    }

    #[test]
    fn solar_uses_actual_not_forecast() {
        // pv_generation_kw=10.0 (forecast), actual_pv_kw=4.0 (observed).
        // Surplus should use actual: 4.0 - 2.0 = 2.0 kW, not 10.0 - 2.0 = 8.0 kW.
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_w: 10000.0,
                actual_pv_w: 4000.0,
                base_load_w: 2000.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_w: 1000.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:surplus");
        assert!((vote.power_w.unwrap() - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn solar_falls_back_when_no_actual() {
        // actual_pv_kw=0.0, so fall back to pv_generation_kw=5.0.
        // Surplus: 5.0 - 1.5 = 3.5 kW.
        let env = TestEnvBuilder::new()
            .with_electrical(ElectricalSummary {
                pv_generation_w: 5000.0,
                actual_pv_w: 0.0,
                base_load_w: 1500.0,
                ..Default::default()
            })
            .build();
        let mut pref = SolarTracking {
            min_charge_rate_w: 1000.0,
        };
        let ctx = make_ctx(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "solar:surplus");
        assert!((vote.power_w.unwrap() - 3500.0).abs() < 1e-9);
    }
}
