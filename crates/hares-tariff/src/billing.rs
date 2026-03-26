use std::collections::VecDeque;

use chrono::{DateTime, Datelike, Duration, TimeZone};
use chrono_tz::Tz;
use hares_types::BillingCycle;

use crate::types::RatchetConfig;

struct DemandWindow {
    samples: Box<[f64]>,
    head: usize,
    count: usize,
    running_sum: f64,
}

impl DemandWindow {
    fn new(capacity: usize) -> Self {
        Self {
            samples: vec![0.0; capacity].into_boxed_slice(),
            head: 0,
            count: 0,
            running_sum: 0.0,
        }
    }

    fn push(&mut self, power_kw: f64) {
        let cap = self.samples.len();
        if cap == 0 {
            return;
        }
        self.running_sum -= self.samples[self.head];
        self.samples[self.head] = power_kw;
        self.running_sum += power_kw;
        self.head = (self.head + 1) % cap;
        if self.count < cap {
            self.count += 1;
        }
    }

    fn average(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.running_sum / self.count as f64
        }
    }
}

pub struct BillingState {
    pub period_start: DateTime<Tz>,
    pub period_end: DateTime<Tz>,
    pub cumulative_import_kwh: f64,
    pub cumulative_export_kwh: f64,
    pub cumulative_energy_cost_usd: f64,
    pub cumulative_export_credit_usd: f64,
    pub peak_demand_kw: f64,
    pub prior_peaks_kw: VecDeque<f64>,
    demand_window: DemandWindow,
    billing_cycle: BillingCycle,
    ratchet_config: Option<RatchetConfig>,
}

fn compute_period_end(start: DateTime<Tz>, cycle: BillingCycle) -> DateTime<Tz> {
    match cycle {
        BillingCycle::Monthly => {
            let (year, month) = if start.month() == 12 {
                (start.year() + 1, 1)
            } else {
                (start.year(), start.month() + 1)
            };
            start
                .timezone()
                .with_ymd_and_hms(year, month, start.day().min(28), 0, 0, 0)
                .single()
                .unwrap_or_else(|| {
                    start
                        .timezone()
                        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
                        .unwrap()
                })
        }
        BillingCycle::Custom(days) => start + Duration::days(days as i64),
    }
}

impl BillingState {
    pub fn new(
        period_start: DateTime<Tz>,
        billing_cycle: BillingCycle,
        demand_window_minutes: u32,
        interval_seconds: u32,
        ratchet_config: Option<RatchetConfig>,
    ) -> Self {
        let capacity = if interval_seconds > 0 {
            (demand_window_minutes as usize * 60) / interval_seconds as usize
        } else {
            1
        };
        let capacity = capacity.max(1);
        let period_end = compute_period_end(period_start, billing_cycle);
        Self {
            period_start,
            period_end,
            cumulative_import_kwh: 0.0,
            cumulative_export_kwh: 0.0,
            cumulative_energy_cost_usd: 0.0,
            cumulative_export_credit_usd: 0.0,
            peak_demand_kw: 0.0,
            prior_peaks_kw: VecDeque::with_capacity(12),
            demand_window: DemandWindow::new(capacity),
            billing_cycle,
            ratchet_config,
        }
    }

    pub fn update(
        &mut self,
        net_power_kw: f64,
        dt_seconds: f64,
        import_price: f64,
        export_price: f64,
    ) {
        let import_kwh = net_power_kw.max(0.0) * dt_seconds / 3600.0;
        let export_kwh = (-net_power_kw).max(0.0) * dt_seconds / 3600.0;
        self.cumulative_import_kwh += import_kwh;
        self.cumulative_export_kwh += export_kwh;
        self.cumulative_energy_cost_usd += import_kwh * import_price;
        self.cumulative_export_credit_usd += export_kwh * export_price;
        self.demand_window.push(net_power_kw.max(0.0));
        self.peak_demand_kw = self.peak_demand_kw.max(self.demand_window.average());
    }

    pub fn effective_peak_kw(&self) -> f64 {
        apply_ratchet(self.peak_demand_kw, &self.prior_peaks_kw, &self.ratchet_config)
    }

    pub fn reset(&mut self, new_period_start: DateTime<Tz>) {
        self.prior_peaks_kw.push_back(self.peak_demand_kw);
        if self.prior_peaks_kw.len() > 12 {
            self.prior_peaks_kw.pop_front();
        }
        self.period_start = new_period_start;
        self.period_end = compute_period_end(new_period_start, self.billing_cycle);
        self.cumulative_import_kwh = 0.0;
        self.cumulative_export_kwh = 0.0;
        self.cumulative_energy_cost_usd = 0.0;
        self.cumulative_export_credit_usd = 0.0;
        self.peak_demand_kw = 0.0;
        self.demand_window = DemandWindow::new(self.demand_window.samples.len());
    }
}

pub struct BillingPeriodSummary {
    pub period_start: DateTime<Tz>,
    pub period_end: DateTime<Tz>,
    pub energy_charge_usd: f64,
    pub demand_charge_usd: f64,
    pub fixed_charge_usd: f64,
    pub export_credit_usd: f64,
    pub net_bill_usd: f64,
    pub peak_demand_kw: f64,
    pub total_import_kwh: f64,
    pub total_export_kwh: f64,
}

fn apply_ratchet(
    current_peak: f64,
    prior_peaks: &VecDeque<f64>,
    ratchet: &Option<RatchetConfig>,
) -> f64 {
    match ratchet {
        Some(rc) if !prior_peaks.is_empty() => {
            let lookback = prior_peaks
                .iter()
                .rev()
                .take(rc.lookback_months as usize);
            let max_prior = lookback.copied().fold(0.0_f64, f64::max);
            current_peak.max(rc.minimum_fraction * max_prior)
        }
        _ => current_peak,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::America::New_York;

    fn make_dt(year: i32, month: u32, day: u32) -> DateTime<Tz> {
        New_York
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .unwrap()
    }

    #[test]
    fn demand_window_rolling_average() {
        let mut w = DemandWindow::new(3);
        w.push(6.0);
        w.push(3.0);
        w.push(9.0);
        assert!((w.average() - 6.0).abs() < 1e-10);
        w.push(12.0);
        // Window now holds [12.0, 3.0, 9.0] -> oldest (6.0) was evicted
        // Actually ring buffer: samples = [12.0, 3.0, 9.0], sum = 24.0, avg = 8.0
        assert!((w.average() - 8.0).abs() < 1e-10);
    }

    #[test]
    fn demand_window_peak_tracking() {
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            300,
            None,
        );
        // 15-min window with 5-min interval = 3 samples
        // Push sequence: 2, 4, 6 -> avg 4.0, then 8, 10, 12 -> avg 10.0
        for &kw in &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0] {
            state.update(kw, 300.0, 0.0, 0.0);
        }
        assert!((state.peak_demand_kw - 10.0).abs() < 1e-10);
    }

    #[test]
    fn billing_state_energy_accumulation() {
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            None,
        );
        // Constant 1 kW for 1 hour (one step of 3600s)
        state.update(1.0, 3600.0, 0.10, 0.0);
        assert!((state.cumulative_import_kwh - 1.0).abs() < 1e-10);
        assert!((state.cumulative_export_kwh).abs() < 1e-10);
        assert!((state.cumulative_energy_cost_usd - 0.10).abs() < 1e-10);
    }

    #[test]
    fn billing_state_export_accumulation() {
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            None,
        );
        // Constant -2 kW for 1 hour
        state.update(-2.0, 3600.0, 0.10, 0.05);
        assert!((state.cumulative_import_kwh).abs() < 1e-10);
        assert!((state.cumulative_export_kwh - 2.0).abs() < 1e-10);
        assert!((state.cumulative_export_credit_usd - 0.10).abs() < 1e-10);
    }

    #[test]
    fn billing_period_closes_at_month_end() {
        let start = make_dt(2025, 1, 1);
        let state = BillingState::new(start, BillingCycle::Monthly, 15, 3600, None);
        let expected_end = make_dt(2025, 2, 1);
        assert_eq!(state.period_end, expected_end);
    }

    #[test]
    fn billing_period_resets_on_close() {
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            None,
        );
        state.update(5.0, 3600.0, 0.10, 0.0);
        assert!(state.cumulative_import_kwh > 0.0);
        assert!(state.peak_demand_kw > 0.0);

        state.reset(make_dt(2025, 2, 1));
        assert!((state.cumulative_import_kwh).abs() < 1e-10);
        assert!((state.cumulative_export_kwh).abs() < 1e-10);
        assert!((state.cumulative_energy_cost_usd).abs() < 1e-10);
        assert!((state.peak_demand_kw).abs() < 1e-10);
        assert_eq!(state.period_start, make_dt(2025, 2, 1));
        assert_eq!(state.period_end, make_dt(2025, 3, 1));
        assert_eq!(state.prior_peaks_kw.len(), 1);
    }

    #[test]
    fn billing_ratchet_applies() {
        let ratchet = RatchetConfig {
            lookback_months: 11,
            minimum_fraction: 0.85,
        };
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            Some(ratchet),
        );
        // Simulate a high prior peak
        state.prior_peaks_kw.push_back(10.0);
        // Current peak is only 2 kW
        state.update(2.0, 3600.0, 0.0, 0.0);
        let effective = state.effective_peak_kw();
        // 0.85 * 10.0 = 8.5 > 2.0, so ratchet applies
        assert!((effective - 8.5).abs() < 1e-10);
    }

    #[test]
    fn billing_ratchet_not_applied_when_current_higher() {
        let ratchet = RatchetConfig {
            lookback_months: 11,
            minimum_fraction: 0.85,
        };
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            300,
            Some(ratchet),
        );
        state.prior_peaks_kw.push_back(5.0);
        // Push enough samples to fill the window with 10 kW
        // 15 min / 5 min = 3 samples
        for _ in 0..3 {
            state.update(10.0, 300.0, 0.0, 0.0);
        }
        let effective = state.effective_peak_kw();
        // 0.85 * 5.0 = 4.25 < 10.0, so current peak wins
        assert!((effective - 10.0).abs() < 1e-10);
    }

    #[test]
    fn billing_prior_peaks_bounded() {
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            None,
        );
        for month in 1..=13u32 {
            state.peak_demand_kw = month as f64;
            let next_start = if month <= 11 {
                make_dt(2025, month + 1, 1)
            } else {
                make_dt(2026, month - 11, 1)
            };
            state.reset(next_start);
        }
        assert_eq!(state.prior_peaks_kw.len(), 12);
        // Oldest (1.0) should have been evicted; front should be 2.0
        assert!((state.prior_peaks_kw[0] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn billing_period_summary_net_bill() {
        let summary = BillingPeriodSummary {
            period_start: make_dt(2025, 1, 1),
            period_end: make_dt(2025, 2, 1),
            energy_charge_usd: 50.0,
            demand_charge_usd: 25.0,
            fixed_charge_usd: 12.50,
            export_credit_usd: 5.0,
            net_bill_usd: 50.0 + 25.0 + 12.50 - 5.0,
            peak_demand_kw: 8.0,
            total_import_kwh: 500.0,
            total_export_kwh: 100.0,
        };
        let expected_net = summary.energy_charge_usd + summary.demand_charge_usd
            + summary.fixed_charge_usd - summary.export_credit_usd;
        assert!((summary.net_bill_usd - expected_net).abs() < 1e-10);
        assert!((summary.net_bill_usd - 82.5).abs() < 1e-10);
    }
}
