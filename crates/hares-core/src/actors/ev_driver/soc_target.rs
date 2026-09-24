//! SocTarget preference -- "charge to X%".

use super::preference::{
    ChargingPreference, DecisionContext, PreferenceVote, needed_charge_hours_to_target,
};

pub struct SocTarget {
    pub target_soc: f64,
    /// Base charging efficiency (dimensionless, e.g. 0.9) used for the
    /// time-to-charge estimate. Temperature degradation is applied on top
    /// inside [`needed_charge_hours_to_target`].
    pub charging_efficiency: f64,
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

    /// Time to charge from the current SOC to the target. Without this
    /// estimate, every strategy whose stack contains only `SocTarget`
    /// (`Immediate`, `Nightly`, `V2H`, `V2G`) reports the "no estimate"
    /// sentinel, which the composer collapses to 0.0 — indistinguishable
    /// from "nothing to charge" while the battery sits well below target.
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
    fn needed_charge_hours_matches_constant_efficiency_calculation_at_epa_baseline() {
        let env = TestEnvBuilder::new().outdoor_temp(22.0).build();
        let pref = SocTarget {
            target_soc: 0.9,
            charging_efficiency: 0.9,
        };
        // Gap 0.4 on a 60 kWh pack = 24 kWh; 24 / (7.2 * 0.9) h, rounded up
        // to whole 1-minute steps.
        let ctx = make_ctx(&env, 0.5);
        let raw: f64 = 24.0 / (7.2 * 0.9);
        let step_hours: f64 = 1.0 / 60.0;
        let expected = (raw / step_hours).ceil() * step_hours;
        let hours = pref.needed_charge_hours(&ctx);
        assert!(
            (hours - expected).abs() < 1e-9,
            "needed_charge_hours ({hours}) should equal the constant-efficiency expected ({expected}) at 22C"
        );
    }

    #[test]
    fn needed_charge_hours_zero_at_or_above_target() {
        let env = TestEnvBuilder::new().outdoor_temp(22.0).build();
        let pref = SocTarget {
            target_soc: 0.9,
            charging_efficiency: 0.9,
        };

        // At target: gap 0 — genuinely nothing to charge.
        let ctx_at = make_ctx(&env, 0.9);
        assert!(
            pref.needed_charge_hours(&ctx_at).abs() < 1e-9,
            "needed_charge_hours must be 0.0 when the SOC gap is zero"
        );

        // Above target: gap clamps to 0 — the estimate is a charge-time, not
        // a discharge-time.
        let ctx_above = make_ctx(&env, 0.95);
        assert!(
            pref.needed_charge_hours(&ctx_above).abs() < 1e-9,
            "needed_charge_hours must be 0.0 when SOC is above target"
        );
    }

    /// Without a usable charge rate the estimate is `INFINITY` ("cannot
    /// charge"), which the composer getter collapses to a finite telemetry
    /// value. The guard must fire before the division: without it, a
    /// negative rate yields negative hours and a zero gap at zero rate
    /// yields NaN — both trip the helper's own finite-non-negative
    /// invariant instead of reporting the sentinel.
    #[test]
    fn needed_charge_hours_infinite_without_charge_rate() {
        let env = TestEnvBuilder::new().outdoor_temp(22.0).build();
        let pref = SocTarget {
            target_soc: 0.9,
            charging_efficiency: 0.9,
        };

        // Negative rate, positive gap: unguarded division goes negative.
        let mut ctx = make_ctx(&env, 0.5);
        ctx.max_charge_kw = -1.0;
        assert_eq!(
            pref.needed_charge_hours(&ctx),
            f64::INFINITY,
            "negative max charge rate must yield INFINITY (cannot charge)"
        );

        // Zero gap at zero rate: unguarded division is 0/0 = NaN.
        let mut ctx = make_ctx(&env, 0.9);
        ctx.max_charge_kw = 0.0;
        assert_eq!(
            pref.needed_charge_hours(&ctx),
            f64::INFINITY,
            "zero gap at zero rate must yield the sentinel, not NaN"
        );
    }

    /// A zero-length timestep skips the round-up-to-step logic and returns
    /// the raw (unrounded) estimate.
    #[test]
    fn needed_charge_hours_unrounded_when_timestep_zero() {
        let env = TestEnvBuilder::new().outdoor_temp(22.0).build();
        let pref = SocTarget {
            target_soc: 0.9,
            charging_efficiency: 0.9,
        };

        let mut ctx = make_ctx(&env, 0.5);
        ctx.time_res_minutes = 0.0;
        let hours = pref.needed_charge_hours(&ctx);
        let raw: f64 = 24.0 / (7.2 * 0.9);
        assert!(
            (hours - raw).abs() < 1e-9,
            "zero timestep must return the raw estimate ({raw}), got {hours}"
        );
    }

    #[test]
    fn score_proportional_to_gap() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocTarget {
            target_soc: 0.9,
            charging_efficiency: 0.9,
        };

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
        let mut pref = SocTarget {
            target_soc: 0.8,
            charging_efficiency: 0.9,
        };

        let ctx = make_ctx(&env, 0.8);
        let vote = pref.score(&ctx);
        assert!(vote.score.abs() < 1e-9);
    }

    #[test]
    fn score_zero_when_above_target() {
        let env = TestEnvBuilder::new().build();
        let mut pref = SocTarget {
            target_soc: 0.9,
            charging_efficiency: 0.9,
        };

        let ctx = make_ctx(&env, 0.95);
        let vote = pref.score(&ctx);
        assert!(vote.score.abs() < 1e-9);
        assert_eq!(vote.target_soc, Some(0.9));
    }
}
