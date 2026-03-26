use chrono::{DateTime, Datelike, Duration, Timelike};
use chrono_tz::Tz;
use hares_types::HaresError;

use crate::billing::{BillingPeriodSummary, BillingState};
use crate::types::{ElectricTariff, ExportMode};

pub struct TariffEvaluator {
    tariff: ElectricTariff,
    price_array: Vec<f64>,
    export_array: Vec<f64>,
    /// Index into a deduplicated period name table per step.
    period_indices: Vec<u16>,
    period_name_table: Vec<String>,
    interval_seconds: u32,
    simulation_start: DateTime<Tz>,
    timezone: Tz,
    step_index: usize,
    billing_state: BillingState,
}

impl TariffEvaluator {
    pub fn new(
        tariff: ElectricTariff,
        simulation_start: DateTime<Tz>,
        simulation_end: DateTime<Tz>,
        interval_seconds: u32,
    ) -> Result<Self, HaresError> {
        if interval_seconds == 0 {
            return Err(HaresError::Tariff(
                "interval_seconds must be > 0".into(),
            ));
        }

        let total_seconds = (simulation_end - simulation_start).num_seconds();
        if total_seconds <= 0 {
            return Err(HaresError::Tariff(
                "simulation_end must be after simulation_start".into(),
            ));
        }

        tariff.validate()?;

        let timezone = simulation_start.timezone();
        let num_steps = total_seconds as usize / interval_seconds as usize;
        let mut price_array = Vec::with_capacity(num_steps);
        let mut export_array = Vec::with_capacity(num_steps);
        let mut period_indices = Vec::with_capacity(num_steps);

        // Intern period names to avoid per-step String allocations.
        let mut period_name_table: Vec<String> = Vec::new();
        // Index 0 = empty string (no match).
        period_name_table.push(String::new());
        for period in &tariff.tou_schedule {
            if !period_name_table.contains(&period.name) {
                period_name_table.push(period.name.clone());
            }
        }

        let intern = |name: &str| -> u16 {
            period_name_table
                .iter()
                .position(|n| n == name)
                .unwrap_or(0) as u16
        };

        for i in 0..num_steps {
            let ts = simulation_start + Duration::seconds(i as i64 * interval_seconds as i64);
            let civil = ts.with_timezone(&timezone);
            let month = civil.month() as u8;
            let weekday = civil.weekday();
            let minute_of_day = civil.hour() as u16 * 60 + civil.minute() as u16;

            let mut matched_period: Option<&str> = None;
            for period in &tariff.tou_schedule {
                if !period.season.contains_month(month) {
                    continue;
                }
                for tw in &period.schedule {
                    if tw.contains(weekday, minute_of_day) {
                        matched_period = Some(&period.name);
                        break;
                    }
                }
                if matched_period.is_some() {
                    break;
                }
            }

            let (import_price, period_idx) = match matched_period {
                Some(name) => {
                    let rate = tariff
                        .energy_rates
                        .iter()
                        .find(|er| er.period_name == name && er.season.contains_month(month))
                        .map(|er| er.rate_per_kwh)
                        .unwrap_or(0.0);
                    (rate, intern(name))
                }
                None => (0.0, 0),
            };

            let export_price = match &tariff.export_rate.mode {
                ExportMode::NetMetering => import_price,
                ExportMode::FlatRate(r) => *r,
                ExportMode::NetBilling => {
                    if let Some(name) = matched_period {
                        tariff
                            .export_rate
                            .tou_credits
                            .iter()
                            .find(|er| {
                                er.period_name == name && er.season.contains_month(month)
                            })
                            .map(|er| er.rate_per_kwh)
                            .unwrap_or(0.0)
                    } else {
                        0.0
                    }
                }
                ExportMode::None => 0.0,
            };

            price_array.push(import_price);
            export_array.push(export_price);
            period_indices.push(period_idx);
        }

        let ratchet_config = tariff
            .demand_rates
            .iter()
            .find_map(|dr| dr.ratchet.clone());

        // Standard US utility 15-minute demand averaging window (FERC/NERC).
        let demand_window_minutes: u32 = 15;
        let billing_state = BillingState::new(
            simulation_start,
            tariff.billing_cycle,
            demand_window_minutes,
            interval_seconds,
            ratchet_config,
        );

        Ok(Self {
            tariff,
            price_array,
            export_array,
            period_indices,
            period_name_table,
            interval_seconds,
            simulation_start,
            timezone,
            step_index: 0,
            billing_state,
        })
    }

    pub fn current_price(&self) -> f64 {
        self.price_array[self.step_index]
    }

    pub fn current_export_price(&self) -> f64 {
        self.export_array[self.step_index]
    }

    pub fn current_period_name(&self) -> &str {
        let idx = self.period_indices[self.step_index] as usize;
        &self.period_name_table[idx]
    }

    /// Advance to the next timestep. Returns `false` if already at the last step.
    pub fn advance(&mut self) -> bool {
        if self.step_index + 1 < self.price_array.len() {
            self.step_index += 1;
            true
        } else {
            false
        }
    }

    pub fn tier_multiplier(&self, cumulative_kwh: f64) -> f64 {
        let ts = self.simulation_start
            + Duration::seconds(self.step_index as i64 * self.interval_seconds as i64);
        let civil = ts.with_timezone(&self.timezone);
        let month = civil.month() as u8;

        for block in &self.tariff.tiered_rates {
            if !block.season.contains_month(month) {
                continue;
            }
            for (i, threshold) in block.thresholds_kwh.iter().enumerate() {
                if cumulative_kwh < *threshold {
                    return block.rates_per_kwh[i];
                }
            }
            return block.rates_per_kwh[block.thresholds_kwh.len()];
        }

        // No tiered block for current season — return the current step's energy price.
        self.price_array[self.step_index]
    }

    /// Returns a subslice of the price array, or `None` if indices are out of bounds.
    pub fn price_slice(&self, start_idx: usize, end_idx: usize) -> Option<&[f64]> {
        self.price_array.get(start_idx..end_idx)
    }

    pub fn step(
        &mut self,
        net_power_kw: f64,
        dt_seconds: f64,
        current_time: DateTime<Tz>,
    ) -> Option<BillingPeriodSummary> {
        let import_price = self.current_price();
        let export_price = self.current_export_price();
        self.billing_state
            .update(net_power_kw, dt_seconds, import_price, export_price);

        let result = if current_time >= self.billing_state.period_end {
            let civil = self.billing_state.period_start.with_timezone(&self.timezone);
            let month = civil.month() as u8;

            let effective_peak = self.billing_state.effective_peak_kw();
            let demand_charge = self.compute_demand_charge(effective_peak, month);

            let days_in_period = (self.billing_state.period_end - self.billing_state.period_start)
                .num_days() as f64;
            let fixed_charge = self.tariff.fixed_charges.monthly_usd
                + self.tariff.fixed_charges.daily_usd * days_in_period;

            let energy_charge = self.billing_state.cumulative_energy_cost_usd;
            let export_credit = self.billing_state.cumulative_export_credit_usd;
            let net_bill = energy_charge + demand_charge + fixed_charge - export_credit;

            let summary = BillingPeriodSummary {
                period_start: self.billing_state.period_start,
                period_end: self.billing_state.period_end,
                energy_charge_usd: energy_charge,
                demand_charge_usd: demand_charge,
                fixed_charge_usd: fixed_charge,
                export_credit_usd: export_credit,
                net_bill_usd: net_bill,
                peak_demand_kw: self.billing_state.peak_demand_kw,
                total_import_kwh: self.billing_state.cumulative_import_kwh,
                total_export_kwh: self.billing_state.cumulative_export_kwh,
            };

            self.billing_state.reset(self.billing_state.period_end);
            Some(summary)
        } else {
            None
        };

        self.advance();
        result
    }

    fn compute_demand_charge(&self, effective_peak_kw: f64, month: u8) -> f64 {
        self.tariff
            .demand_rates
            .iter()
            .filter(|dr| dr.season.contains_month(month))
            .map(|dr| effective_peak_kw * dr.rate_per_kw)
            .sum()
    }

    pub fn billing_state(&self) -> &BillingState {
        &self.billing_state
    }

    pub fn step_index(&self) -> usize {
        self.step_index
    }

    pub fn total_steps(&self) -> usize {
        self.price_array.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::America::New_York;
    use hares_types::{DayFilter, SeasonFilter, TimeWindow, TouPeriod};

    use crate::types::{EnergyRate, ExportRate, FixedCharges, TieredBlock};

    const SUMMER_PEAK: f64 = 0.35;
    const SUMMER_OFFPEAK: f64 = 0.10;
    const WINTER_PEAK: f64 = 0.25;
    const WINTER_OFFPEAK: f64 = 0.08;

    fn test_tariff() -> ElectricTariff {
        ElectricTariff {
            name: Some("test-2period".into()),
            tou_schedule: vec![
                TouPeriod {
                    name: "on-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Weekdays, 960, 1260, 0.0)],
                    season: SeasonFilter::All,
                },
                TouPeriod {
                    name: "off-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                    season: SeasonFilter::All,
                },
            ],
            energy_rates: vec![
                EnergyRate {
                    period_name: "on-peak".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: SUMMER_PEAK,
                },
                EnergyRate {
                    period_name: "on-peak".into(),
                    season: SeasonFilter::Winter,
                    rate_per_kwh: WINTER_PEAK,
                },
                EnergyRate {
                    period_name: "off-peak".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: SUMMER_OFFPEAK,
                },
                EnergyRate {
                    period_name: "off-peak".into(),
                    season: SeasonFilter::Winter,
                    rate_per_kwh: WINTER_OFFPEAK,
                },
            ],
            tiered_rates: vec![
                TieredBlock {
                    season: SeasonFilter::Summer,
                    thresholds_kwh: vec![500.0],
                    rates_per_kwh: vec![0.10, 0.20],
                },
                TieredBlock {
                    season: SeasonFilter::Winter,
                    thresholds_kwh: vec![700.0],
                    rates_per_kwh: vec![0.08, 0.15],
                },
            ],
            ..Default::default()
        }
    }

    fn make_start(year: i32, month: u32, day: u32) -> DateTime<Tz> {
        New_York
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .unwrap()
    }

    fn make_evaluator(
        tariff: ElectricTariff,
        start: DateTime<Tz>,
        end: DateTime<Tz>,
        interval: u32,
    ) -> TariffEvaluator {
        TariffEvaluator::new(tariff, start, end, interval).unwrap()
    }

    #[test]
    fn evaluator_hourly_array_length() {
        let ev = make_evaluator(test_tariff(), make_start(2025, 1, 1), make_start(2026, 1, 1), 3600);
        assert_eq!(ev.total_steps(), 8760);
    }

    #[test]
    fn evaluator_15min_array_length() {
        let ev = make_evaluator(test_tariff(), make_start(2025, 1, 1), make_start(2026, 1, 1), 900);
        assert_eq!(ev.total_steps(), 35040);
    }

    #[test]
    fn evaluator_summer_peak_price() {
        // July 7, 2025 is a Monday. 5pm = minute 1020, within on-peak [960, 1260).
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 17, 0, 0).unwrap();
        let ev = make_evaluator(test_tariff(), start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_price(), SUMMER_PEAK);
        assert_eq!(ev.current_period_name(), "on-peak");
    }

    #[test]
    fn evaluator_winter_offpeak_price() {
        // Jan 4, 2025 is a Saturday. Midnight = off-peak.
        let start = New_York.with_ymd_and_hms(2025, 1, 4, 0, 0, 0).unwrap();
        let ev = make_evaluator(test_tariff(), start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_price(), WINTER_OFFPEAK);
        assert_eq!(ev.current_period_name(), "off-peak");
    }

    #[test]
    fn evaluator_season_boundary() {
        // May 30, 2025 is a Friday (winter). June 2, 2025 is a Monday (summer).
        let may = New_York.with_ymd_and_hms(2025, 5, 30, 17, 0, 0).unwrap();
        let ev_may = make_evaluator(test_tariff(), may, may + Duration::hours(1), 3600);
        assert_eq!(ev_may.current_price(), WINTER_PEAK);

        let jun = New_York.with_ymd_and_hms(2025, 6, 2, 17, 0, 0).unwrap();
        let ev_jun = make_evaluator(test_tariff(), jun, jun + Duration::hours(1), 3600);
        assert_eq!(ev_jun.current_price(), SUMMER_PEAK);
    }

    #[test]
    fn evaluator_dst_spring_forward() {
        // 2025 DST spring forward: March 9. The day has 23 hours.
        let start = New_York.with_ymd_and_hms(2025, 3, 9, 0, 0, 0).unwrap();
        let end = New_York.with_ymd_and_hms(2025, 3, 10, 0, 0, 0).unwrap();
        assert_eq!((end - start).num_seconds(), 23 * 3600);
        let ev = make_evaluator(test_tariff(), start, end, 3600);
        assert_eq!(ev.total_steps(), 23);
    }

    #[test]
    fn evaluator_dst_fall_back() {
        // 2025 DST fall back: November 2. The day has 25 hours.
        let start = New_York.with_ymd_and_hms(2025, 11, 2, 0, 0, 0).unwrap();
        let end = New_York.with_ymd_and_hms(2025, 11, 3, 0, 0, 0).unwrap();
        assert_eq!((end - start).num_seconds(), 25 * 3600);
        let ev = make_evaluator(test_tariff(), start, end, 3600);
        assert_eq!(ev.total_steps(), 25);
    }

    #[test]
    fn evaluator_tier_multiplier_first_tier() {
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(test_tariff(), start, start + Duration::hours(1), 3600);
        assert_eq!(ev.tier_multiplier(0.0), 0.10);
        assert_eq!(ev.tier_multiplier(499.9), 0.10);
    }

    #[test]
    fn evaluator_tier_multiplier_second_tier() {
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(test_tariff(), start, start + Duration::hours(1), 3600);
        assert_eq!(ev.tier_multiplier(500.0), 0.20);
        assert_eq!(ev.tier_multiplier(1000.0), 0.20);
    }

    #[test]
    fn evaluator_price_slice() {
        let ev = make_evaluator(test_tariff(), make_start(2025, 1, 1), make_start(2025, 1, 2), 3600);
        let slice = ev.price_slice(0, 3).unwrap();
        assert_eq!(slice.len(), 3);

        // Out-of-bounds returns None.
        assert!(ev.price_slice(0, 100).is_none());
        assert!(ev.price_slice(20, 10).is_none());
    }

    #[test]
    fn evaluator_flat_rate_tariff() {
        let flat_tariff = ElectricTariff {
            name: Some("flat".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.12,
            }],
            ..Default::default()
        };
        let ev = make_evaluator(flat_tariff, make_start(2025, 1, 1), make_start(2026, 1, 1), 3600);
        assert_eq!(ev.total_steps(), 8760);
        for price in &ev.price_array {
            assert_eq!(*price, 0.12);
        }
    }

    #[test]
    fn evaluator_no_match_returns_zero() {
        // Tariff with only a summer on-peak period — winter timestamps have no match.
        let partial = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "summer-peak".into(),
                schedule: vec![TimeWindow::new(DayFilter::Weekdays, 960, 1260, 0.0)],
                season: SeasonFilter::Summer,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "summer-peak".into(),
                season: SeasonFilter::Summer,
                rate_per_kwh: 0.35,
            }],
            ..Default::default()
        };
        // January weekday — no matching period.
        let start = New_York.with_ymd_and_hms(2025, 1, 6, 17, 0, 0).unwrap();
        let ev = make_evaluator(partial, start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_price(), 0.0);
        assert_eq!(ev.current_period_name(), "");
    }

    #[test]
    fn evaluator_advance_returns_false_at_end() {
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut ev = make_evaluator(test_tariff(), start, start + Duration::hours(3), 3600);
        assert_eq!(ev.total_steps(), 3);
        assert_eq!(ev.step_index(), 0);
        assert!(ev.advance());
        assert_eq!(ev.step_index(), 1);
        assert!(ev.advance());
        assert_eq!(ev.step_index(), 2);
        assert!(!ev.advance());
        assert_eq!(ev.step_index(), 2);
    }

    #[test]
    fn evaluator_export_net_metering() {
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "peak".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.30,
            }],
            export_rate: ExportRate {
                mode: ExportMode::NetMetering,
                tou_credits: vec![],
            },
            ..Default::default()
        };
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_export_price(), 0.30);
    }

    #[test]
    fn evaluator_export_flat_rate() {
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "peak".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.30,
            }],
            export_rate: ExportRate {
                mode: ExportMode::FlatRate(0.05),
                tou_credits: vec![],
            },
            ..Default::default()
        };
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_export_price(), 0.05);
    }

    #[test]
    fn evaluator_export_none() {
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "peak".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.30,
            }],
            export_rate: ExportRate {
                mode: ExportMode::None,
                tou_credits: vec![],
            },
            ..Default::default()
        };
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_export_price(), 0.0);
    }

    #[test]
    fn evaluator_export_net_billing() {
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: "peak".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.30,
            }],
            export_rate: ExportRate {
                mode: ExportMode::NetBilling,
                tou_credits: vec![EnergyRate {
                    period_name: "peak".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.08,
                }],
            },
            fixed_charges: FixedCharges::default(),
            ..Default::default()
        };
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert_eq!(ev.current_export_price(), 0.08);
    }
}
