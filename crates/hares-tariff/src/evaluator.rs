use chrono::{DateTime, Datelike, Duration, Timelike};
use chrono_tz::Tz;
use hares_types::HaresError;

use crate::billing::{BillingPeriodSummary, BillingState};
use crate::types::{ElectricTariff, ExportMode};

pub struct TariffEvaluator {
    tariff: ElectricTariff,
    price_array: Vec<f64>,
    export_array: Vec<f64>,
    /// Index into a deduplicated period name table per step (energy TOU).
    period_indices: Vec<u16>,
    /// Index into period name table per step for demand TOU periods.
    demand_period_indices: Vec<u16>,
    /// Precomputed civil month (1-12) for each timestep.
    months: Vec<u8>,
    period_name_table: Vec<String>,
    interval_seconds: u32,
    simulation_start: DateTime<Tz>,
    step_index: usize,
    billing_state: BillingState,
    finished: bool,
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
        let mut demand_period_indices = Vec::with_capacity(num_steps);
        let mut months = Vec::with_capacity(num_steps);

        // Intern period names to avoid per-step String allocations.
        let mut period_name_table: Vec<String> = Vec::new();
        // Index 0 = empty string (no match).
        period_name_table.push(String::new());
        for period in &tariff.tou_schedule {
            if !period_name_table.contains(&period.name) {
                period_name_table.push(period.name.clone());
            }
        }
        for period in &tariff.demand_tou_schedule {
            if !period_name_table.contains(&period.name) {
                period_name_table.push(period.name.clone());
            }
        }
        for dr in &tariff.demand_rates {
            if let Some(name) = &dr.period_name {
                if !period_name_table.contains(name) {
                    period_name_table.push(name.clone());
                }
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

            // Resolve demand TOU period for this timestep.
            let demand_idx = if tariff.demand_tou_schedule.is_empty() {
                period_idx
            } else {
                let mut matched = None;
                for period in &tariff.demand_tou_schedule {
                    if !period.season.contains_month(month) {
                        continue;
                    }
                    for tw in &period.schedule {
                        if tw.contains(weekday, minute_of_day) {
                            matched = Some(&period.name);
                            break;
                        }
                    }
                    if matched.is_some() {
                        break;
                    }
                }
                matched.map(|n| intern(n)).unwrap_or(0)
            };

            price_array.push(import_price);
            export_array.push(export_price);
            period_indices.push(period_idx);
            demand_period_indices.push(demand_idx);
            months.push(month);
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
            period_name_table.len(),
        );

        Ok(Self {
            tariff,
            price_array,
            export_array,
            period_indices,
            demand_period_indices,
            months,
            period_name_table,
            interval_seconds,
            simulation_start,
            step_index: 0,
            billing_state,
            finished: false,
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
        let month = self.months[self.step_index];

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
        if self.finished {
            return None;
        }
        let import_price = self.current_price();
        let export_price = self.current_export_price();
        let period_idx = self.period_indices[self.step_index];
        let demand_period_idx = self.demand_period_indices[self.step_index];
        self.billing_state.update(
            net_power_kw,
            dt_seconds,
            import_price,
            export_price,
            period_idx,
            demand_period_idx,
        );

        let result = if current_time >= self.billing_state.period_end {
            let month = self.billing_state.period_start.month() as u8;

            let demand_charge = self.compute_demand_charge(month);

            let days_in_period = (self.billing_state.period_end - self.billing_state.period_start)
                .num_days() as f64;
            let fixed_charge = self.tariff.fixed_charges.monthly_usd
                + self.tariff.fixed_charges.daily_usd * days_in_period;

            let energy_charge = self.billing_state.cumulative_energy_cost_usd;
            let export_credit = self.billing_state.cumulative_export_credit_usd;

            let summary = BillingPeriodSummary::new(
                self.billing_state.period_start,
                self.billing_state.period_end,
                energy_charge,
                demand_charge,
                fixed_charge,
                export_credit,
                self.tariff.minimum_charge,
                self.billing_state.peak_demand_kw,
                self.billing_state.cumulative_import_kwh,
                self.billing_state.cumulative_export_kwh,
            );

            self.billing_state.reset(self.billing_state.period_end);
            Some(summary)
        } else {
            None
        };

        if !self.advance() {
            self.finished = true;
        }
        result
    }

    fn compute_demand_charge(&self, month: u8) -> f64 {
        let global_peak = self.billing_state.effective_peak_kw();
        self.tariff
            .demand_rates
            .iter()
            .filter(|dr| dr.season.contains_month(month))
            .map(|dr| {
                let peak = match &dr.period_name {
                    None => global_peak,
                    Some(name) => {
                        let idx = self
                            .period_name_table
                            .iter()
                            .position(|n| n == name)
                            .unwrap_or(0) as u16;
                        self.billing_state
                            .effective_peak_for_period(idx, &dr.ratchet)
                    }
                };
                peak * dr.rate_per_kw
            })
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

    pub fn interval_seconds(&self) -> u32 {
        self.interval_seconds
    }

    pub fn simulation_start(&self) -> DateTime<Tz> {
        self.simulation_start
    }

    /// Emit the final partial billing period. Call after the last `step()`.
    /// Returns `None` if already finalized or if no charges accumulated.
    pub fn finalize(&mut self) -> Option<BillingPeriodSummary> {
        let bs = &self.billing_state;
        if bs.cumulative_import_kwh() == 0.0
            && bs.cumulative_export_kwh() == 0.0
            && bs.peak_demand_kw() == 0.0
        {
            return None;
        }
        let month = bs.period_start().month() as u8;
        let demand_charge = self.compute_demand_charge(month);
        let days_in_period = (bs.period_end() - bs.period_start()).num_days() as f64;
        let fixed_charge = self.tariff.fixed_charges.monthly_usd
            + self.tariff.fixed_charges.daily_usd * days_in_period;
        let energy_charge = bs.cumulative_energy_cost_usd();
        let export_credit = bs.cumulative_export_credit_usd();
        let peak = bs.peak_demand_kw();
        let import = bs.cumulative_import_kwh();
        let export = bs.cumulative_export_kwh();
        let start = bs.period_start();
        let end = bs.period_end();

        // Reset billing state so a second call returns None.
        self.billing_state.reset(end);

        Some(BillingPeriodSummary::new(
            start,
            end,
            energy_charge,
            demand_charge,
            fixed_charge,
            export_credit,
            self.tariff.minimum_charge,
            peak,
            import,
            export,
        ))
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

    /// Flat tariff covering all hours/seasons at a known rate, for step() tests.
    fn flat_tariff(rate: f64) -> ElectricTariff {
        ElectricTariff {
            name: Some("flat-step".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: rate,
            }],
            ..Default::default()
        }
    }

    /// Run step() for every timestep, passing end-of-step time as current_time.
    /// The billing period closes when current_time >= period_end, so we pass
    /// the timestamp at the END of each interval.
    fn run_all_steps(
        ev: &mut TariffEvaluator,
        power_fn: impl Fn(usize) -> f64,
    ) -> Vec<BillingPeriodSummary> {
        let total = ev.total_steps();
        let interval = ev.interval_seconds;
        let start = ev.simulation_start;
        let mut summaries = Vec::new();
        for i in 0..total {
            let step_end = start + Duration::seconds((i as i64 + 1) * interval as i64);
            if let Some(s) = ev.step(power_fn(i), interval as f64, step_end) {
                summaries.push(s);
            }
        }
        summaries
    }

    #[test]
    fn evaluator_step_accumulates_energy() {
        let start = make_start(2025, 1, 1);
        let end = start + Duration::hours(24);
        let interval = 3600u32;
        let mut ev = make_evaluator(flat_tariff(0.12), start, end, interval);

        run_all_steps(&mut ev, |_| 2.0);
        assert!(
            (ev.billing_state().cumulative_import_kwh() - 48.0).abs() < 1e-10,
            "expected 48.0 kWh, got {}",
            ev.billing_state().cumulative_import_kwh()
        );
    }

    #[test]
    fn evaluator_step_billing_period_closes() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 3, 1);
        let interval = 3600u32;
        let monthly_fixed = 15.0;
        let mut tariff = flat_tariff(0.12);
        tariff.fixed_charges = FixedCharges {
            monthly_usd: monthly_fixed,
            daily_usd: 0.0,
        };
        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |_| 2.0);

        assert_eq!(summaries.len(), 2, "expected exactly 2 billing period closes");

        let s0 = &summaries[0];
        assert!(s0.energy_charge_usd > 0.0);
        assert!(s0.total_import_kwh > 0.0);
        assert!(
            (s0.fixed_charge_usd - monthly_fixed).abs() < 1e-10,
            "expected fixed_charge_usd={monthly_fixed}, got {}",
            s0.fixed_charge_usd
        );
    }

    #[test]
    fn evaluator_step_demand_charge_in_summary() {
        use crate::types::DemandRate;

        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let demand_rate_per_kw = 10.0;
        let mut tariff = flat_tariff(0.12);
        tariff.demand_rates = vec![DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: demand_rate_per_kw,
            ratchet: None,
        }];
        let mut ev = make_evaluator(tariff, start, end, interval);
        let half = ev.total_steps() / 2;

        let summaries = run_all_steps(&mut ev, |i| if i < half { 5.0 } else { 1.0 });

        let summary = summaries.into_iter().next().expect("billing period should close");
        assert!(
            summary.demand_charge_usd > 0.0,
            "demand_charge_usd should be > 0"
        );
        assert!(
            summary.peak_demand_kw >= 4.9,
            "peak_demand_kw should be near 5.0, got {}",
            summary.peak_demand_kw
        );
    }

    #[test]
    fn evaluator_tou_demand_charge_uses_period_peak() {
        use crate::types::DemandRate;

        // Two TOU periods: on-peak (weekdays 16:00-21:00) and off-peak (all other).
        // Load 10 kW during on-peak, 2 kW during off-peak.
        // Demand rate with period_name "on-peak" should use the on-peak period peak (~10 kW),
        // not the global peak (which is also ~10 kW here). We verify by adding a second
        // demand rate for "off-peak" and checking it uses only the off-peak peak (~2 kW).
        let start = make_start(2025, 1, 1); // Wednesday
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;

        let tariff = ElectricTariff {
            name: Some("tou-demand".into()),
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
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.30,
                },
                EnergyRate {
                    period_name: "off-peak".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.10,
                },
            ],
            demand_rates: vec![
                DemandRate {
                    period_name: Some("on-peak".into()),
                    season: SeasonFilter::All,
                    rate_per_kw: 10.0,
                    ratchet: None,
                },
                DemandRate {
                    period_name: Some("off-peak".into()),
                    season: SeasonFilter::All,
                    rate_per_kw: 5.0,
                    ratchet: None,
                },
            ],
            ..Default::default()
        };

        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |i| {
            // Compute the civil hour for this step to decide load.
            let step_start = start + Duration::seconds(i as i64 * interval as i64);
            let civil = step_start.with_timezone(&New_York);
            let minute_of_day = civil.hour() as u16 * 60 + civil.minute() as u16;
            let is_weekday = matches!(
                civil.weekday(),
                chrono::Weekday::Mon
                    | chrono::Weekday::Tue
                    | chrono::Weekday::Wed
                    | chrono::Weekday::Thu
                    | chrono::Weekday::Fri
            );
            if is_weekday && (960..1260).contains(&minute_of_day) {
                10.0 // on-peak: 10 kW
            } else {
                2.0 // off-peak: 2 kW
            }
        });

        let s = summaries.into_iter().next().expect("billing period should close");

        // On-peak demand charge: ~10 kW * $10/kW = ~$100
        // Off-peak demand charge: ~2 kW * $5/kW = ~$10
        // Total demand charge: ~$110
        // If bug existed (using global peak for both), it would be 10*10 + 10*5 = $150
        let on_peak_contribution = 10.0 * 10.0;
        let off_peak_contribution = 2.0 * 5.0;
        let expected_demand = on_peak_contribution + off_peak_contribution;

        assert!(
            (s.demand_charge_usd - expected_demand).abs() < 1.0,
            "TOU demand charge should be ~{expected_demand}, got {}. \
             If using global peak for both periods, would be {}",
            s.demand_charge_usd,
            10.0 * 10.0 + 10.0 * 5.0,
        );
    }

    #[test]
    fn evaluator_step_export_credit_in_summary() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let mut tariff = flat_tariff(0.30);
        tariff.export_rate = ExportRate {
            mode: ExportMode::NetMetering,
            tou_credits: vec![],
        };
        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |_| -3.0);

        let summary = summaries.into_iter().next().expect("billing period should close");
        assert!(
            summary.export_credit_usd > 0.0,
            "export_credit_usd should be > 0"
        );
        assert!(
            summary.total_export_kwh > 0.0,
            "total_export_kwh should be > 0"
        );
    }

    #[test]
    fn evaluator_step_net_bill_arithmetic() {
        use crate::types::DemandRate;

        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let mut tariff = flat_tariff(0.15);
        tariff.fixed_charges = FixedCharges {
            monthly_usd: 12.0,
            daily_usd: 0.0,
        };
        tariff.demand_rates = vec![DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: 8.0,
            ratchet: None,
        }];
        tariff.export_rate = ExportRate {
            mode: ExportMode::NetMetering,
            tou_credits: vec![],
        };
        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |i| if i % 2 == 0 { 3.0 } else { -1.0 });

        let s = summaries.into_iter().next().expect("billing period should close");
        let expected = s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd
            - s.export_credit_usd;
        assert!(
            (s.net_bill_usd - expected).abs() < 1e-10,
            "net_bill_usd={} != energy({}) + demand({}) + fixed({}) - export({})",
            s.net_bill_usd,
            s.energy_charge_usd,
            s.demand_charge_usd,
            s.fixed_charge_usd,
            s.export_credit_usd
        );
    }

    #[test]
    fn evaluator_new_rejects_zero_interval() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let result = TariffEvaluator::new(flat_tariff(0.12), start, end, 0);
        assert!(result.is_err());
    }

    #[test]
    fn evaluator_new_rejects_end_before_start() {
        let start = make_start(2025, 6, 1);
        let end = make_start(2025, 1, 1);
        let result = TariffEvaluator::new(flat_tariff(0.12), start, end, 3600);
        assert!(result.is_err());

        // Also test equal start and end.
        let result_eq = TariffEvaluator::new(flat_tariff(0.12), start, start, 3600);
        assert!(result_eq.is_err());
    }

    #[test]
    fn evaluator_tier_multiplier_no_tiers_returns_price() {
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let tariff = flat_tariff(0.12);
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.tier_multiplier(0.0) - 0.12).abs() < 1e-10,
            "with no tiers, tier_multiplier should return current_price (0.12), got {}",
            ev.tier_multiplier(0.0)
        );
        assert!(
            (ev.tier_multiplier(999.0) - 0.12).abs() < 1e-10,
            "with no tiers, tier_multiplier should return current_price regardless of kwh"
        );
    }

    #[test]
    fn evaluator_minimum_charge_enforced() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let mut tariff = flat_tariff(0.12);
        tariff.minimum_charge = Some(50.0);

        let mut ev = make_evaluator(tariff, start, end, interval);

        // Very low usage: 0.1 kW for the whole month → ~74.4 kWh → ~$8.93 energy
        // With no demand/fixed/export, raw bill < $50 minimum
        let summaries = run_all_steps(&mut ev, |_| 0.1);

        let s = summaries.into_iter().next().expect("billing period should close");
        let raw = s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd
            - s.export_credit_usd;
        assert!(raw < 50.0, "raw bill should be below minimum, got {raw}");
        assert!(
            (s.net_bill_usd - 50.0).abs() < 1e-10,
            "net_bill_usd should be clamped to minimum_charge $50, got {}",
            s.net_bill_usd,
        );
    }

    #[test]
    fn evaluator_minimum_charge_not_applied_when_bill_exceeds() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let mut tariff = flat_tariff(0.12);
        tariff.minimum_charge = Some(10.0);

        let mut ev = make_evaluator(tariff, start, end, interval);

        // 5 kW for the whole month → ~3720 kWh → ~$446 energy, well above $10 min
        let summaries = run_all_steps(&mut ev, |_| 5.0);

        let s = summaries.into_iter().next().expect("billing period should close");
        let raw = s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd
            - s.export_credit_usd;
        assert!(
            (s.net_bill_usd - raw).abs() < 1e-10,
            "net_bill_usd should equal raw bill when above minimum, got {} vs {raw}",
            s.net_bill_usd,
        );
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

    // C1: URDB tariff with demand_tou_schedule produces nonzero TOU demand charges.
    #[test]
    fn evaluator_demand_tou_schedule_produces_nonzero_demand_charge() {
        use crate::types::DemandRate;

        // Monday January 6, 2025 — a weekday.
        let start = make_start(2025, 1, 6);
        let end = make_start(2025, 2, 6);
        let interval = 3600u32;

        let tariff = ElectricTariff {
            name: Some("urdb-demand-tou".into()),
            tou_schedule: vec![TouPeriod {
                name: "peak".into(),
                // Weekdays 16:00–21:00 (minutes 960–1260).
                schedule: vec![TimeWindow::new(DayFilter::Weekdays, 960, 1260, 0.0)],
                season: SeasonFilter::All,
            }],
            demand_tou_schedule: vec![TouPeriod {
                name: "demand_0".into(),
                schedule: vec![TimeWindow::new(DayFilter::Weekdays, 960, 1260, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "peak".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.30,
            }],
            demand_rates: vec![DemandRate {
                period_name: Some("demand_0".into()),
                season: SeasonFilter::All,
                rate_per_kw: 10.0,
                ratchet: None,
            }],
            ..Default::default()
        };

        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |i| {
            let step_ts = start + Duration::seconds(i as i64 * interval as i64);
            let civil = step_ts.with_timezone(&New_York);
            let minute_of_day = civil.hour() as u16 * 60 + civil.minute() as u16;
            let is_weekday = matches!(
                civil.weekday(),
                chrono::Weekday::Mon
                    | chrono::Weekday::Tue
                    | chrono::Weekday::Wed
                    | chrono::Weekday::Thu
                    | chrono::Weekday::Fri
            );
            if is_weekday && (960..1260).contains(&minute_of_day) {
                10.0
            } else {
                1.0
            }
        });

        let s = summaries.into_iter().next().expect("billing period should close");
        assert!(
            s.demand_charge_usd > 0.0,
            "demand_charge_usd should be > 0 when load is present during demand TOU window, got {}",
            s.demand_charge_usd
        );
    }

    // M2: finalize() returns Some on first call and None on second call.
    #[test]
    fn evaluator_finalize_double_call_returns_none() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let mut ev = make_evaluator(flat_tariff(0.12), start, end, interval);

        // Accumulate some load so finalize() has something to return.
        let step_end = start + Duration::seconds(interval as i64);
        ev.step(5.0, interval as f64, step_end);

        let first = ev.finalize();
        assert!(first.is_some(), "first finalize() should return Some");

        let second = ev.finalize();
        assert!(second.is_none(), "second finalize() should return None after billing state was reset");
    }

    // L1: step() after simulation end returns None.
    #[test]
    fn evaluator_step_after_end_returns_none() {
        // One step simulation: start → start + 1h, interval = 1h → 1 step total.
        let start = make_start(2025, 1, 1);
        let end = start + Duration::hours(1);
        let interval = 3600u32;
        let mut ev = make_evaluator(flat_tariff(0.12), start, end, interval);

        assert_eq!(ev.total_steps(), 1);

        // The single valid step.
        let step_end = start + Duration::seconds(interval as i64);
        let _ = ev.step(1.0, interval as f64, step_end);

        // Any subsequent call must return None — the evaluator is finished.
        let result = ev.step(1.0, interval as f64, step_end + Duration::hours(1));
        assert!(result.is_none(), "step() after simulation end should return None");
    }
}
