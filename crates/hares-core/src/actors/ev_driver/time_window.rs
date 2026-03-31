//! TimeWindowPref — "only charge during these hours".
//!
//! Delegates to `hares_types::TimeWindow::contains()` for the actual
//! time-matching logic (including midnight wrapping and day filters).

use chrono::Datelike;
use hares_types::{DayFilter, TimeWindow};

use super::preference::{ChargingPreference, Constraint, DecisionContext, PreferenceVote};

pub struct TimeWindowPref {
    window: TimeWindow,
}

impl TimeWindowPref {
    pub fn from_hours(start_hour: f64, end_hour: f64) -> Self {
        let start_minute = (start_hour * 60.0) as u16;
        let end_minute = (end_hour * 60.0).min(1440.0) as u16;
        Self::from_time_window(TimeWindow::new(
            DayFilter::Any,
            start_minute,
            end_minute,
            0.0,
        ))
    }

    pub fn from_time_window(window: TimeWindow) -> Self {
        Self { window }
    }
}

impl ChargingPreference for TimeWindowPref {
    fn constraint(&mut self, ctx: &DecisionContext) -> Constraint {
        let weekday = ctx.env.current_time.weekday();
        if !self.window.contains(weekday, ctx.current_minute) {
            Constraint::Override(PreferenceVote::idle("time_window:outside"))
        } else {
            Constraint::Inactive
        }
    }

    fn score(&mut self, _ctx: &DecisionContext) -> PreferenceVote {
        PreferenceVote {
            target_soc: None,
            power_kw: None,
            departure_hour: None,
            min_soc: None,
            max_soc: None,
            score: 0.0,
            label: "time_window:active",
        }
    }

    fn name(&self) -> &'static str {
        "TimeWindowPref"
    }
}

#[cfg(test)]
mod tests {
    use chrono::Weekday;

    use super::*;
    use crate::actor::testing::TestEnvBuilder;

    fn make_ctx(env: &hares_types::EnvironmentState, minute: u16) -> DecisionContext<'_> {
        DecisionContext {
            current_soc: 0.5,
            capacity_kwh: 60.0,
            max_charge_kw: 7.2,
            max_discharge_kw: 5.0,
            env,
            current_minute: minute,
            next_departure_minute: None,
            time_res_minutes: 1.0,
        }
    }

    #[test]
    fn overrides_idle_outside_window() {
        let env = TestEnvBuilder::new().build();
        // Window 22:00 - 06:00 (1320-360), current 12:00 (720)
        let mut pref = TimeWindowPref::from_hours(22.0, 6.0);
        let ctx = make_ctx(&env, 720);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "time_window:outside");
                assert!(vote.power_kw.is_none());
            }
            Constraint::Inactive => panic!("expected Override outside window"),
        }
    }

    #[test]
    fn inactive_inside_window_no_wrap() {
        let env = TestEnvBuilder::new().build();
        // Window 08:00 - 18:00 (480-1080), current 12:00 (720)
        let mut pref = TimeWindowPref::from_hours(8.0, 18.0);
        let ctx = make_ctx(&env, 720);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
    }

    #[test]
    fn inactive_inside_midnight_wrap() {
        let env = TestEnvBuilder::new().build();
        // Window 22:00 - 06:00, current 23:00 (1380)
        let mut pref = TimeWindowPref::from_hours(22.0, 6.0);
        let ctx = make_ctx(&env, 1380);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
    }

    #[test]
    fn inactive_early_morning_in_wrap() {
        let env = TestEnvBuilder::new().build();
        // Window 22:00 - 06:00, current 03:00 (180)
        let mut pref = TimeWindowPref::from_hours(22.0, 6.0);
        let ctx = make_ctx(&env, 180);
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));
    }

    #[test]
    fn overrides_outside_no_wrap() {
        let env = TestEnvBuilder::new().build();
        // Window 08:00 - 18:00, current 20:00 (1200)
        let mut pref = TimeWindowPref::from_hours(8.0, 18.0);
        let ctx = make_ctx(&env, 1200);
        match pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "time_window:outside");
                assert!(vote.power_kw.is_none());
            }
            Constraint::Inactive => panic!("expected Override outside window"),
        }
    }

    #[test]
    fn respects_day_filter_from_time_window() {
        let env = TestEnvBuilder::new().build();
        // Weekdays only, 08:00-18:00. Default TestEnvBuilder is 2026-01-01 = Thursday.
        let tw = TimeWindow::new(DayFilter::Weekdays, 480, 1080, 0.0);
        let mut pref = TimeWindowPref::from_time_window(tw);
        let ctx = make_ctx(&env, 720); // Thursday noon
        assert!(matches!(pref.constraint(&ctx), Constraint::Inactive));

        // Same window, but weekend day
        let weekend_tw = TimeWindow::new(DayFilter::Day(Weekday::Sat), 480, 1080, 0.0);
        let mut weekend_pref = TimeWindowPref::from_time_window(weekend_tw);
        // Thursday is not Saturday — should override idle
        match weekend_pref.constraint(&ctx) {
            Constraint::Override(vote) => {
                assert_eq!(vote.label, "time_window:outside");
            }
            Constraint::Inactive => panic!("expected Override for wrong day"),
        }
    }
}
