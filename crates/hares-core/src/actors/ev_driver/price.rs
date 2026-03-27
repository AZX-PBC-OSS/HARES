//! PriceOptimizer — TOU-aware charging with day-boundary cached thresholds.

use std::sync::Arc;

use chrono::Datelike;

use super::preference::{ChargingPreference, DecisionContext, PreferenceVote};

pub struct PriceOptimizer {
    charge_percentile: f64,
    discharge_percentile: f64,
    price_schedule: Option<Arc<[f64]>>,
    steps_per_day: usize,
    charge_threshold: f64,
    discharge_threshold: f64,
    current_day: u32,
}

impl PriceOptimizer {
    pub fn new(
        charge_percentile: f64,
        discharge_percentile: f64,
        price_schedule: Option<Arc<[f64]>>,
        steps_per_day: usize,
    ) -> Self {
        Self {
            charge_percentile,
            discharge_percentile,
            price_schedule,
            steps_per_day,
            charge_threshold: 0.0,
            discharge_threshold: f64::INFINITY,
            current_day: u32::MAX,
        }
    }

    fn ensure_thresholds(&mut self, ctx: &DecisionContext) {
        let day = ctx.env.current_time.ordinal0();
        if day == self.current_day {
            return;
        }
        self.current_day = day;

        let Some(prices) = &self.price_schedule else {
            return;
        };

        let day_of_year = day as usize;
        let start = day_of_year * self.steps_per_day;
        let end = (start + self.steps_per_day).min(prices.len());

        if start >= prices.len() || start >= end {
            return;
        }

        let today = &prices[start..end];
        self.charge_threshold = compute_percentile(today, self.charge_percentile);
        self.discharge_threshold = compute_percentile(today, self.discharge_percentile);
    }
}

fn compute_percentile(prices: &[f64], percentile: f64) -> f64 {
    if prices.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = prices.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((percentile * (sorted.len() - 1) as f64).round() as usize).min(sorted.len() - 1);
    sorted[idx]
}

impl ChargingPreference for PriceOptimizer {
    fn score(&mut self, ctx: &DecisionContext) -> PreferenceVote {
        self.ensure_thresholds(ctx);

        let price = ctx.env.price_signal.electricity_price.unwrap_or(0.0);

        if price <= self.charge_threshold {
            // Cheap price — encourage charging
            let urgency = if self.charge_threshold > 0.0 {
                1.0 - (price / self.charge_threshold).min(1.0)
            } else {
                1.0
            };
            PreferenceVote {
                target_soc: None,
                power_kw: Some(ctx.max_charge_kw),
                departure_hour: None,
                min_soc: None,
                max_soc: None,
                score: 2.0 + urgency,
                label: "price:charge",
            }
        } else if price >= self.discharge_threshold {
            // Expensive price — encourage discharge
            let urgency = if self.discharge_threshold > 0.0 {
                ((price / self.discharge_threshold) - 1.0).min(1.0)
            } else {
                1.0
            };
            PreferenceVote {
                target_soc: None,
                power_kw: Some(-ctx.max_discharge_kw),
                departure_hour: None,
                min_soc: None,
                max_soc: None,
                score: 2.0 + urgency,
                label: "price:discharge",
            }
        } else {
            PreferenceVote::idle("price:neutral")
        }
    }

    fn name(&self) -> &'static str {
        "PriceOptimizer"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::testing::TestEnvBuilder;
    use hares_types::PriceSignal;

    fn make_ctx_with_price(
        env: &hares_types::EnvironmentState,
    ) -> DecisionContext<'_> {
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
    fn charges_at_low_price() {
        // 24 steps/day, prices for day 0
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.01).collect();
        let mut pref = PriceOptimizer::new(0.25, 0.75, Some(prices.into()), 24);

        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.02),
                ..Default::default()
            })
            .build();
        let ctx = make_ctx_with_price(&env);
        let vote = pref.score(&ctx);

        assert!(vote.score > 0.0);
        assert_eq!(vote.label, "price:charge");
        assert!(vote.power_kw.unwrap() > 0.0);
    }

    #[test]
    fn discharges_at_high_price() {
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.01).collect();
        let mut pref = PriceOptimizer::new(0.25, 0.75, Some(prices.into()), 24);

        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.20),
                ..Default::default()
            })
            .build();
        let ctx = make_ctx_with_price(&env);
        let vote = pref.score(&ctx);

        assert!(vote.score > 0.0);
        assert_eq!(vote.label, "price:discharge");
        assert!(vote.power_kw.unwrap() < 0.0);
    }

    #[test]
    fn neutral_at_mid_price() {
        let prices: Vec<f64> = (0..24).map(|i| i as f64 * 0.01).collect();
        let mut pref = PriceOptimizer::new(0.25, 0.75, Some(prices.into()), 24);

        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.10),
                ..Default::default()
            })
            .build();
        let ctx = make_ctx_with_price(&env);
        let vote = pref.score(&ctx);

        assert_eq!(vote.label, "price:neutral");
        assert!(vote.power_kw.is_none());
    }

    #[test]
    fn day_boundary_caching() {
        let mut prices = vec![0.0; 48];
        // Day 0: all 0.10
        for p in prices.iter_mut().take(24) {
            *p = 0.10;
        }
        // Day 1: all 0.50
        for p in prices.iter_mut().skip(24) {
            *p = 0.50;
        }

        let mut pref = PriceOptimizer::new(0.25, 0.75, Some(prices.into()), 24);

        // Day 0
        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.10),
                ..Default::default()
            })
            .build();
        let ctx = make_ctx_with_price(&env);
        let vote = pref.score(&ctx);
        // All prices same = threshold is 0.10, so price == threshold is charge
        assert_eq!(vote.label, "price:charge");

        // Verify the day was cached
        assert_eq!(pref.current_day, 0);

        // Day 1: prices are all 0.50, so thresholds should change.
        let env_day1 = TestEnvBuilder::new()
            .date(2026, 1, 2) // day ordinal = 1
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.50),
                ..Default::default()
            })
            .build();
        let ctx_day1 = make_ctx_with_price(&env_day1);
        let vote_day1 = pref.score(&ctx_day1);
        assert_eq!(pref.current_day, 1, "day should advance to 1");
        // All day-1 prices are 0.50, so charge_threshold = 0.50.
        // price 0.50 <= 0.50 → charge
        assert_eq!(vote_day1.label, "price:charge");
    }

    #[test]
    fn compute_percentile_empty_returns_zero() {
        assert!((compute_percentile(&[], 0.5) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn compute_percentile_single_element() {
        assert!((compute_percentile(&[0.42], 0.5) - 0.42).abs() < 1e-9);
        assert!((compute_percentile(&[0.42], 0.0) - 0.42).abs() < 1e-9);
        assert!((compute_percentile(&[0.42], 1.0) - 0.42).abs() < 1e-9);
    }

    #[test]
    fn compute_percentile_at_zero_and_one() {
        let prices = [0.10, 0.20, 0.30, 0.40, 0.50];
        assert!((compute_percentile(&prices, 0.0) - 0.10).abs() < 1e-9);
        assert!((compute_percentile(&prices, 1.0) - 0.50).abs() < 1e-9);
    }

    #[test]
    fn no_price_schedule_uses_defaults() {
        let mut pref = PriceOptimizer::new(0.25, 0.75, None, 24);

        let env = TestEnvBuilder::new()
            .with_price_signal(PriceSignal {
                electricity_price: Some(0.10),
                ..Default::default()
            })
            .build();
        let ctx = make_ctx_with_price(&env);
        let vote = pref.score(&ctx);

        // charge_threshold defaults to 0.0, so 0.10 > 0.0
        // discharge_threshold defaults to INF, so 0.10 < INF
        assert_eq!(vote.label, "price:neutral");
    }
}
