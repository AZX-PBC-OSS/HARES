//! DepartureDeadline -- hard constraint near departure time.

use chrono::Datelike;
use hares_types::DepartureConstraint;

use super::preference::{ChargingPreference, Constraint, DecisionContext, PreferenceVote};

pub struct DepartureDeadline {
    pub schedule: Vec<DepartureConstraint>,
    pub target_soc: f64,
    pub efficiency: f64,
    /// When time_to_departure < buffer_hours AND soc < target, force max-rate
    /// charging regardless of price. Fires earlier than the physical-urgency
    /// override (which only fires when there isn't enough time to charge).
    pub buffer_hours: f64,
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
    /// Rounds up to the nearest timestep to avoid underestimating charge time.
    ///
    /// Applies `temp_efficiency_multiplier` to the base charging efficiency to
    /// account for temperature-dependent degradation of charging acceptance
    /// (cold weather reduces BMS acceptance rate, thermal conditioning draws
    /// power, and internal resistance increases). The multiplier is the same
    /// piecewise-linear fleet-average curve from `efficiency.rs`, originally
    /// calibrated for driving energy consumption (AAA 2019, Geotab 2020,
    /// DOE/Argonne 2024, Recurrent Auto) but applicable to charging because
    /// the same physical mechanisms (electrochemical kinetics, resistive
    /// heating, thermal management) degrade both driving and charging efficiency
    /// at low temperatures.
    fn needed_charge_hours(&self, ctx: &DecisionContext) -> f64 {
        let soc_gap = (self.target_soc - ctx.current_soc).max(0.0);
        let energy_kwh = soc_gap * ctx.capacity_kwh;
        if ctx.max_charge_kw <= 0.0 || self.efficiency <= 0.0 {
            return f64::INFINITY;
        }
        let temp_mult =
            super::efficiency::temp_efficiency_multiplier(ctx.env.weather.outdoor_temp_c);
        let effective_efficiency = self.efficiency / temp_mult;
        let raw_hours = energy_kwh / (ctx.max_charge_kw * effective_efficiency);
        let hours = if ctx.time_res_minutes > 0.0 {
            let step_hours = ctx.time_res_minutes / 60.0;
            (raw_hours / step_hours).ceil() * step_hours
        } else {
            raw_hours
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                hours.is_finite() && hours >= 0.0,
                "needed_charge_hours: result is not a finite non-negative number; got hours={hours}, soc_gap={soc_gap}, energy_kwh={energy_kwh}, max_charge_kw={}, effective_efficiency={effective_efficiency}",
                ctx.max_charge_kw,
            );
        }

        hours
    }

    /// Minutes remaining until departure.
    fn minutes_until_departure(&self, ctx: &DecisionContext, departure_minute: u32) -> f64 {
        let dm = departure_minute as i32;
        let cm = ctx.current_minute as i32;
        let diff = dm - cm;
        if diff > 0 {
            diff as f64
        } else {
            (diff + 1440) as f64
        }
    }

    /// Resolve departure minute: prefer the actor-provided next_departure_minute
    /// (which accounts for today's rolled event), fall back to schedule scan.
    fn resolve_departure(&self, ctx: &DecisionContext) -> Option<(u32, f64)> {
        if let Some(dep_min) = ctx.next_departure_minute {
            let dc = self.find_today(ctx)?;
            return Some((dep_min as u32, dc.target_soc));
        }
        let dc = self.find_today(ctx)?;
        Some((dc.departure_minute, dc.target_soc))
    }
}

impl ChargingPreference for DepartureDeadline {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        let Some((departure_minute, dep_target_soc)) = self.resolve_departure(ctx) else {
            return Constraint::Inactive;
        };

        let minutes_left = self.minutes_until_departure(ctx, departure_minute);
        let hours_left = minutes_left / 60.0;
        let needed = self.needed_charge_hours(ctx);

        // Buffer window: if within buffer_hours of departure and still need
        // charge, force max-rate charging regardless of price optimality.
        if self.buffer_hours > 0.0
            && hours_left <= self.buffer_hours
            && ctx.current_soc < dep_target_soc
        {
            return Constraint::Override(PreferenceVote {
                target_soc: Some(dep_target_soc),
                power_kw: Some(ctx.max_charge_kw),
                departure_hour: Some(departure_minute as f64 / 60.0),
                min_soc: None,
                max_soc: None,
                score: 10.0,
                label: "departure:buffer",
            });
        }

        // Safety margin: 20% buffer
        if hours_left < needed * 1.2 {
            Constraint::Override(PreferenceVote {
                target_soc: Some(dep_target_soc),
                power_kw: Some(ctx.max_charge_kw),
                departure_hour: Some(departure_minute as f64 / 60.0),
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
        let Some((departure_minute, dep_target_soc)) = self.resolve_departure(ctx) else {
            return PreferenceVote::idle("departure:no_schedule");
        };

        let minutes_left = self.minutes_until_departure(ctx, departure_minute);
        let needed = self.needed_charge_hours(ctx);
        let total_available = minutes_left / 60.0;

        let urgency = if total_available > 0.0 {
            (1.0 - (total_available - needed) / total_available).clamp(0.0, 1.0)
        } else {
            1.0
        };

        PreferenceVote {
            target_soc: Some(dep_target_soc),
            power_kw: None,
            departure_hour: Some(departure_minute as f64 / 60.0),
            min_soc: None,
            max_soc: None,
            score: urgency,
            label: "departure:planned",
        }
    }

    fn needed_charge_hours(&self, ctx: &DecisionContext) -> f64 {
        DepartureDeadline::needed_charge_hours(self, ctx)
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
            efficiency: 0.9,
            buffer_hours: 0.0,
        }
    }

    fn make_ctx(env: &hares_types::EnvironmentState, soc: f64, minute: u16) -> DecisionContext<'_> {
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
        // Default env temp is 10°C → temp_mult=1.11, effective_eff=0.9/1.11=0.811
        // Need to charge from 0.2 to 0.9: 0.7*60/(7.2*0.811) ≈ 7.19h
        // 60 min << 7.19 * 1.2 * 60 = 518 min => urgent
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
        // Default env temp is 10°C → temp_mult=1.11, effective_eff=0.9/1.11=0.811
        // Need to charge from 0.8 to 0.9: 0.1*60/(7.2*0.811) ≈ 1.03h
        // 660 min >> 1.03 * 1.2 * 60 = 74 min => not urgent
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
            efficiency: 0.9,
            buffer_hours: 0.0,
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
        assert!(
            vote.score > 0.0,
            "should have positive urgency across midnight wrap"
        );
    }

    #[test]
    fn no_schedule_match_returns_idle() {
        let env = TestEnvBuilder::new().build();
        let mut pref = DepartureDeadline {
            schedule: vec![], // empty schedule
            target_soc: 0.9,
            efficiency: 0.9,
            buffer_hours: 0.0,
        };

        let ctx = make_ctx(&env, 0.5, 360);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
        let vote = pref.score(&ctx);
        assert_eq!(vote.label, "departure:no_schedule");
    }

    // Finding 1: buffer_hours changes charging behavior
    #[test]
    fn departure_buffer_changes_charging_behavior() {
        let env = TestEnvBuilder::new().hour(4).build(); // 04:00

        // Departure at 07:00, SOC 0.5 needing charge to 0.9.
        // needed = 0.4 * 60 / (7.2 * 0.9) ≈ 3.7h, 3h available.
        // buffer=0: only urgency check: 3h < 3.7 * 1.2 = 4.44h → Override(urgent)
        // But let's pick a scenario where urgency does NOT fire but buffer does:
        // SOC 0.8, target 0.9. Default env (04:00) temp=10°C → effective_eff=0.811
        // needed = 0.1 * 60 / (7.2*0.811) ≈ 1.03h
        // 3h > 1.03 * 1.2 = 1.24h → urgency Inactive.
        // buffer=0: Inactive. buffer=4: 3h <= 4h AND 0.8 < 0.9 → Override(buffer).

        let mut no_buffer = DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 420,
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            efficiency: 0.9,
            buffer_hours: 0.0,
        };
        let mut with_buffer = DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 420,
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            efficiency: 0.9,
            buffer_hours: 4.0,
        };

        // current_minute=240 (04:00), departure=420 (07:00), 180 min = 3h
        let ctx = make_ctx(&env, 0.8, 240);

        let result_no_buffer = no_buffer.constraint(&ctx);
        let result_with_buffer = with_buffer.constraint(&ctx);

        assert!(
            matches!(result_no_buffer, Constraint::Inactive),
            "buffer=0 should be Inactive (no urgency, no buffer)"
        );
        match result_with_buffer {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "departure:buffer");
            }
            Constraint::Inactive => panic!("buffer=4 should Override within buffer window"),
        }
    }

    // Finding 2: capacity_kwh sensitivity
    #[test]
    fn capacity_kwh_changes_urgency_threshold() {
        let env = TestEnvBuilder::new().hour(5).build();

        // SOC gap 0.5, max_charge_kw=7.2, time_to_departure=2h (120 min)
        // Default env temp is 10°C → temp_mult=1.11, effective_eff=0.9/1.11=0.811
        // Small battery (10 kWh): needed = 0.5*10/(7.2*0.811) ≈ 0.86h
        //   2h > 0.86 * 1.2 = 1.03h → Inactive
        // Large battery (100 kWh): needed = 0.5*100/(7.2*0.811) ≈ 8.59h
        //   2h < 8.59 * 1.2 = 10.3h → Override

        let mut pref = DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 420, // 07:00
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            efficiency: 0.9,
            buffer_hours: 0.0,
        };

        // current_minute=300 (05:00), departure=420, 120 min = 2h
        let ctx_small = DecisionContext {
            current_soc: 0.4,
            capacity_kwh: 10.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 300,
            next_departure_minute: Some(420),
            time_res_minutes: 1.0,
        };

        let ctx_large = DecisionContext {
            current_soc: 0.4,
            capacity_kwh: 100.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 300,
            next_departure_minute: Some(420),
            time_res_minutes: 1.0,
        };

        let result_small = pref.constraint(&ctx_small);
        let result_large = pref.constraint(&ctx_large);

        assert!(
            matches!(result_small, Constraint::Inactive),
            "small battery (10 kWh) should have enough time: Inactive"
        );
        assert!(
            matches!(result_large, Constraint::Override(_)),
            "large battery (100 kWh) should be urgent: Override"
        );
    }

    // Finding 3: next_departure_minute overrides schedule
    #[test]
    fn next_departure_minute_overrides_schedule_minute() {
        let env = TestEnvBuilder::new().hour(5).build();

        let mut pref = DepartureDeadline {
            schedule: vec![DepartureConstraint {
                day_filter: DayFilter::Any,
                departure_minute: 480, // 08:00 schedule
                target_soc: 0.9,
            }],
            target_soc: 0.9,
            efficiency: 0.9,
            buffer_hours: 0.0,
        };

        // Context B: next_departure_minute=None, falls back to schedule 480 (08:00)
        // 180 min to departure (3h). temp at 10°C → effective_eff=0.811
        // needed = 0.1*60/(7.2*0.811)≈1.03h, 3h > 1.03*1.2=1.24h → Inactive
        let ctx_b = DecisionContext {
            current_soc: 0.8,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 300,
            next_departure_minute: None,
            time_res_minutes: 1.0,
        };

        // Context A with same SOC=0.8: next_departure=360, 60 min left
        // temp at 10°C → effective_eff=0.811, needed=0.1*60/(7.2*0.811)≈1.03h
        // 1h < 1.03 * 1.2 = 1.24h → Override
        let ctx_a_high_soc = DecisionContext {
            current_soc: 0.8,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env: &env,
            current_minute: 300,
            next_departure_minute: Some(360),
            time_res_minutes: 1.0,
        };

        let result_a = pref.constraint(&ctx_a_high_soc);
        let result_b = pref.constraint(&ctx_b);

        assert!(
            matches!(result_a, Constraint::Override(_)),
            "next_departure_minute=360 (1h away) should trigger urgency Override"
        );
        assert!(
            matches!(result_b, Constraint::Inactive),
            "fallback to schedule minute=480 (3h away) should be Inactive"
        );
    }

    /// Verify that `needed_charge_hours` accounts for cold-weather charging
    /// degradation: at -10°C the estimate should be at least 30% higher than
    /// at the EPA baseline 22°C for the same SOC gap.
    #[test]
    fn needed_charge_hours_increases_in_cold() {
        use crate::actor::testing::TestEnvBuilder;

        let pref = make_departure_pref();

        // Build environments at 22°C (EPA baseline) and -10°C (cold).
        let env_warm = TestEnvBuilder::new().hour(5).outdoor_temp(22.0).build();
        let env_cold = TestEnvBuilder::new().hour(5).outdoor_temp(-10.0).build();

        // Same SOC gap (0.5), capacity, and max charge rate.
        let ctx_warm = make_ctx(&env_warm, 0.4, 300);
        let ctx_cold = make_ctx(&env_cold, 0.4, 300);

        let hours_warm = pref.needed_charge_hours(&ctx_warm);
        let hours_cold = pref.needed_charge_hours(&ctx_cold);

        assert!(
            hours_cold > hours_warm * 1.30,
            "cold (-10°C) hours ({hours_cold:.3}) must be at least 30% greater than warm (22°C) hours ({hours_warm:.3})"
        );
    }

    /// At 22°C (EPA baseline), temp_efficiency_multiplier ≈ 1.0, so
    /// needed_charge_hours should match the constant-efficiency calculation.
    #[test]
    fn needed_charge_hours_unchanged_at_epa_baseline() {
        use crate::actor::testing::TestEnvBuilder;

        let pref = make_departure_pref();
        let env = TestEnvBuilder::new().hour(5).outdoor_temp(22.0).build();
        let ctx = make_ctx(&env, 0.4, 300);

        let hours = pref.needed_charge_hours(&ctx);

        // At 22°C: temp_mult = 1.0, effective_efficiency = 0.9.
        // soc_gap = 0.5, energy_kwh = 0.5 * 60 = 30 kWh.
        // raw_hours = 30 / (7.2 * 0.9) = 4.6296...
        // step_hours = 1/60 ≈ 0.01667, ceil(4.6296 / 0.01667) = 278, 278 * 0.01667 = 4.6333
        let expected_raw: f64 = 30.0 / (7.2 * 0.9);
        let step_hours: f64 = 1.0 / 60.0;
        let expected = (expected_raw / step_hours).ceil() * step_hours;
        assert!(
            (hours - expected).abs() < 1e-9,
            "at 22°C, needed_charge_hours ({hours}) should equal constant-efficiency expected ({expected})"
        );
    }
}
