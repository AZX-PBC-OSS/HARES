//! DepartureDeadline — hard constraint near departure time.

use chrono::Datelike;
use hares_types::DepartureConstraint;

use super::preference::{
    ChargingPreference, Constraint, DecisionContext, PreferenceVote,
};

pub struct DepartureDeadline {
    pub schedule: Vec<DepartureConstraint>,
    pub target_soc: f64,
    pub efficiency: f64,
}

impl DepartureDeadline {
    /// Find the matching departure constraint for today.
    fn find_today(&self, ctx: &DecisionContext) -> Option<&DepartureConstraint> {
        let weekday = ctx.env.current_time.weekday();
        self.schedule
            .iter()
            .find(|dc| dc.day_filter.matches(weekday))
    }

    /// Hours needed to charge from current SOC to target at max rate.
    fn needed_charge_hours(&self, current_soc: f64) -> f64 {
        let soc_gap = (self.target_soc - current_soc).max(0.0);
        let energy_kwh = soc_gap * self.capacity_kwh;
        if self.max_charge_kw <= 0.0 || self.efficiency <= 0.0 {
            return f64::INFINITY;
        }
        energy_kwh / (self.max_charge_kw * self.efficiency)
    }

    /// Minutes remaining until departure.
    fn minutes_until_departure(&self, ctx: &DecisionContext, departure_minute: u32) -> f64 {
        let dm = departure_minute as i32;
        let cm = ctx.current_minute as i32;
        let diff = dm - cm;
        if diff > 0 { diff as f64 } else { (diff + 1440) as f64 }
    }
}

impl ChargingPreference for DepartureDeadline {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        let Some(dc) = self.find_today(ctx) else {
            return Constraint::Inactive;
        };

        let minutes_left = self.minutes_until_departure(ctx, dc.departure_minute);
        let hours_left = minutes_left / 60.0;
        let needed = self.needed_charge_hours(ctx.current_soc);

        // Safety margin: 20% buffer
        if hours_left < needed * 1.2 {
            Constraint::Override(PreferenceVote {
                target_soc: Some(dc.target_soc),
                power_kw: Some(self.max_charge_kw),
                departure_hour: Some(dc.departure_minute as f64 / 60.0),
                min_soc: None,
                max_soc: None,
                score: 10.0,
                label: "departure:urgent",
            })
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        let Some(dc) = self.find_today(ctx) else {
            return PreferenceVote::idle("departure:no_schedule");
        };

        let minutes_left = self.minutes_until_departure(ctx, dc.departure_minute);
        let needed = self.needed_charge_hours(ctx.current_soc);
        let total_available = minutes_left / 60.0;

        let urgency = if total_available > 0.0 {
            (1.0 - (total_available - needed) / total_available).clamp(0.0, 1.0)
        } else {
            1.0
        };

        PreferenceVote {
            target_soc: Some(dc.target_soc),
            power_kw: None,
            departure_hour: Some(dc.departure_minute as f64 / 60.0),
            min_soc: None,
            max_soc: None,
            score: urgency,
            label: "departure:planned",
        }
    }

    fn name(&self) -> &'static str {
        "DepartureDeadline"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;
    use hares_types::DayFilter;

    fn make_departure_pref() -> DepartureDeadline {
        DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 420, // 7:00 AM
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            efficiency: 0.9,
        }
    }

    fn make_ctx(
        env: &hares_types::EnvironmentState,
        soc: f64,
        minute: u16,
    ) -> DecisionContext<'_> {
        DecisionContext {
            current_soc: soc,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env,
            current_minute: minute,
            next_departure_minute: Some(420),
            time_res_minutes: 1.0,
        }
    }

    #[test]
    fn override_when_urgent() {
        let env = TestEnvBuilder::new().hour(6).build();
        let mut pref = make_departure_pref();

        // At 06:00 (360 min), departure at 07:00 (420 min) = 60 min left
        // Need to charge from 0.2 to 0.9 = 0.7 * 60 / (7.2 * 0.9) = 6.48 hours
        // 60 min < 6.48 * 1.2 * 60 = 467 min => urgent
        let ctx = make_ctx(&env, 0.2, 360);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "departure:urgent");
                assert_eq!(vote.target_soc, Some(0.9));
            }
            Constraint::Inactive => panic!("expected Override"),
        }
    }

    #[test]
    fn inactive_when_plenty_of_time() {
        let env = TestEnvBuilder::new().hour(20).build();
        let mut pref = make_departure_pref();

        // At 20:00 (1200 min), departure at 07:00 (420 min) next day = 660 min left
        // Need to charge from 0.8 to 0.9 = 0.1 * 60 / (7.2 * 0.9) = 0.926 hours
        // 660 min >> 0.926 * 1.2 * 60 = 66.7 min => not urgent
        let ctx = make_ctx(&env, 0.8, 1200);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
    }

    #[test]
    fn score_increases_with_urgency() {
        let env = TestEnvBuilder::new().build();
        let mut pref = make_departure_pref();

        // Far from departure with low SOC
        let ctx_far = make_ctx(&env, 0.5, 0); // midnight, 420 min until departure
        let vote_far = pref.score(&ctx_far);

        // Close to departure with same SOC
        let ctx_near = make_ctx(&env, 0.5, 360); // 6 AM, 60 min until departure
        let vote_near = pref.score(&ctx_near);

        assert!(
            vote_near.score > vote_far.score,
            "near urgency ({}) should be strictly greater than far urgency ({})",
            vote_near.score,
            vote_far.score,
        );
    }

    #[test]
    fn departure_midnight_wrap() {
        let env = TestEnvBuilder::new().hour(23).build();
        let mut pref = DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 120, // 02:00
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            efficiency: 0.9,
        };

        // current_minute=1400 (23:20), departure_minute=120 (02:00)
        // Should see ~160 min remaining (wrapping through midnight), not negative.
        let ctx = make_ctx(&env, 0.5, 1400);
        let minutes = pref.minutes_until_departure(&ctx, 120);
        assert!(
            (minutes - 160.0).abs() < 1.0,
            "midnight wrap should give ~160 min remaining, got {minutes}"
        );

        // Verify score doesn't return idle (there is time pressure)
        let vote = pref.score(&ctx);
        assert!(vote.score > 0.0, "should have positive urgency across midnight wrap");
    }

    #[test]
    fn no_schedule_match_returns_idle() {
        let env = TestEnvBuilder::new().build();
        let mut pref = DepartureDeadline {
            schedule: vec![], // empty schedule
            target_soc: 0.9,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            efficiency: 0.9,
        };

        let ctx = make_ctx(&env, 0.5, 360);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        let vote = pref.score(&ctx);
        assert_eq!(vote.label, "departure:no_schedule");
    }
}
