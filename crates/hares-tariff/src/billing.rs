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
    push_count: u64,
}

impl DemandWindow {
    fn new(capacity: usize) -> Self {
        Self {
            samples: vec![0.0; capacity].into_boxed_slice(),
            head: 0,
            count: 0,
            running_sum: 0.0,
            push_count: 0,
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
        self.push_count += 1;
        if self.push_count.is_multiple_of(1000) {
            self.running_sum = self.samples.iter().sum();
        }
    }

    fn average(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.running_sum / self.count as f64
        }
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.head = 0;
        self.count = 0;
        self.running_sum = 0.0;
        // Resetting push_count is intentional: there's no accumulated drift to
        // correct after a full reset, so the periodic recomputation guard restarts.
        self.push_count = 0;
    }
}

pub struct BillingState {
    pub(crate) period_start: DateTime<Tz>,
    pub(crate) period_end: DateTime<Tz>,
    pub(crate) cumulative_import_kwh: f64,
    pub(crate) cumulative_export_kwh: f64,
    pub(crate) cumulative_energy_cost_usd: f64,
    pub(crate) cumulative_export_credit_usd: f64,
    pub(crate) peak_demand_kw: f64,
    /// Per-period peak demand (indexed by period name table index from evaluator).
    pub(crate) period_peak_demand_kw: Vec<f64>,
    pub(crate) prior_peaks_kw: VecDeque<f64>,
    /// Per-period prior peak history for TOU demand ratchet.
    prior_period_peaks: Vec<VecDeque<f64>>,
    demand_window: DemandWindow,
    billing_cycle: BillingCycle,
    ratchet_config: Option<RatchetConfig>,
    max_prior_periods: usize,
}

fn days_in_month(year: i32, month: u32) -> u32 {
    use chrono::NaiveDate;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next = NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .expect("valid NaiveDate for day=1");
    let this = NaiveDate::from_ymd_opt(year, month, 1)
        .expect("valid NaiveDate for day=1");
    (next - this).num_days() as u32
}

fn compute_period_end(start: DateTime<Tz>, cycle: BillingCycle) -> DateTime<Tz> {
    match cycle {
        BillingCycle::Monthly => {
            let (year, month) = if start.month() == 12 {
                (start.year() + 1, 1u32)
            } else {
                (start.year(), start.month() + 1)
            };
            let max_day = days_in_month(year, month);
            let day = start.day().min(max_day);
            start
                .timezone()
                .with_ymd_and_hms(year, month, day, 0, 0, 0)
                .earliest()
                .expect("computed billing period date must be valid after day clamp")
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
        num_tou_periods: usize,
    ) -> Self {
        let capacity = if interval_seconds > 0 {
            (demand_window_minutes as usize * 60) / interval_seconds as usize
        } else {
            1
        };
        let capacity = capacity.max(1);
        let period_end = compute_period_end(period_start, billing_cycle);
        let max_prior_periods = match (&ratchet_config, billing_cycle) {
            (Some(rc), BillingCycle::Custom(days)) if days > 0 => {
                let lookback_days = rc.lookback_months as usize * 31;
                lookback_days.div_ceil(days as usize)
            }
            (Some(rc), _) => rc.lookback_months as usize,
            (None, _) => 12,
        };
        Self {
            period_start,
            period_end,
            cumulative_import_kwh: 0.0,
            cumulative_export_kwh: 0.0,
            cumulative_energy_cost_usd: 0.0,
            cumulative_export_credit_usd: 0.0,
            peak_demand_kw: 0.0,
            period_peak_demand_kw: vec![0.0; num_tou_periods],
            prior_peaks_kw: VecDeque::with_capacity(max_prior_periods),
            prior_period_peaks: (0..num_tou_periods)
                .map(|_| VecDeque::with_capacity(max_prior_periods))
                .collect(),
            demand_window: DemandWindow::new(capacity),
            billing_cycle,
            ratchet_config,
            max_prior_periods,
        }
    }

    pub fn update(
        &mut self,
        net_power_kw: f64,
        dt_seconds: f64,
        import_price: f64,
        export_price: f64,
        period_idx: u16,
        demand_period_idx: u16,
    ) {
        let import_kwh = net_power_kw.max(0.0) * dt_seconds / 3600.0;
        let export_kwh = (-net_power_kw).max(0.0) * dt_seconds / 3600.0;
        self.cumulative_import_kwh += import_kwh;
        self.cumulative_export_kwh += export_kwh;
        self.cumulative_energy_cost_usd += import_kwh * import_price;
        self.cumulative_export_credit_usd += export_kwh * export_price;
        self.demand_window.push(net_power_kw.max(0.0));
        let avg = self.demand_window.average();
        self.peak_demand_kw = self.peak_demand_kw.max(avg);
        if let Some(slot) = self.period_peak_demand_kw.get_mut(period_idx as usize) {
            *slot = slot.max(avg);
        }
        if demand_period_idx != period_idx {
            if let Some(slot) = self.period_peak_demand_kw.get_mut(demand_period_idx as usize) {
                *slot = slot.max(avg);
            }
        }
    }

    pub fn period_start(&self) -> DateTime<Tz> {
        self.period_start
    }

    pub fn period_end(&self) -> DateTime<Tz> {
        self.period_end
    }

    pub fn cumulative_import_kwh(&self) -> f64 {
        self.cumulative_import_kwh
    }

    pub fn cumulative_export_kwh(&self) -> f64 {
        self.cumulative_export_kwh
    }

    pub fn peak_demand_kw(&self) -> f64 {
        self.peak_demand_kw
    }

    pub fn cumulative_energy_cost_usd(&self) -> f64 {
        self.cumulative_energy_cost_usd
    }

    pub fn cumulative_export_credit_usd(&self) -> f64 {
        self.cumulative_export_credit_usd
    }

    pub fn effective_peak_kw(&self) -> f64 {
        apply_ratchet(self.peak_demand_kw, &self.prior_peaks_kw, &self.ratchet_config)
    }

    pub fn peak_for_period(&self, period_idx: u16) -> f64 {
        self.period_peak_demand_kw
            .get(period_idx as usize)
            .copied()
            .unwrap_or(0.0)
    }

    pub fn effective_peak_for_period(
        &self,
        period_idx: u16,
        ratchet: &Option<RatchetConfig>,
    ) -> f64 {
        let current = self.peak_for_period(period_idx);
        let priors = self
            .prior_period_peaks
            .get(period_idx as usize)
            .map(|d| d as &VecDeque<f64>);
        match priors {
            Some(p) => apply_ratchet(current, p, ratchet),
            None => current,
        }
    }

    pub fn reset(&mut self, new_period_start: DateTime<Tz>) {
        self.prior_peaks_kw.push_back(self.peak_demand_kw);
        if self.prior_peaks_kw.len() > self.max_prior_periods {
            self.prior_peaks_kw.pop_front();
        }
        for (i, peak) in self.period_peak_demand_kw.iter().enumerate() {
            if let Some(deque) = self.prior_period_peaks.get_mut(i) {
                deque.push_back(*peak);
                if deque.len() > self.max_prior_periods {
                    deque.pop_front();
                }
            }
        }
        self.period_start = new_period_start;
        self.period_end = compute_period_end(new_period_start, self.billing_cycle);
        self.cumulative_import_kwh = 0.0;
        self.cumulative_export_kwh = 0.0;
        self.cumulative_energy_cost_usd = 0.0;
        self.cumulative_export_credit_usd = 0.0;
        self.peak_demand_kw = 0.0;
        self.period_peak_demand_kw.fill(0.0);
        self.demand_window.reset();
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

impl BillingPeriodSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        period_start: DateTime<Tz>,
        period_end: DateTime<Tz>,
        energy_charge_usd: f64,
        demand_charge_usd: f64,
        fixed_charge_usd: f64,
        export_credit_usd: f64,
        minimum_charge: Option<f64>,
        peak_demand_kw: f64,
        total_import_kwh: f64,
        total_export_kwh: f64,
    ) -> Self {
        let raw = energy_charge_usd + demand_charge_usd + fixed_charge_usd - export_credit_usd;
        let net_bill_usd = match minimum_charge {
            Some(min) if raw < min => min,
            _ => raw,
        };
        Self {
            period_start,
            period_end,
            energy_charge_usd,
            demand_charge_usd,
            fixed_charge_usd,
            export_credit_usd,
            net_bill_usd,
            peak_demand_kw,
            total_import_kwh,
            total_export_kwh,
        }
    }
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
        // Ring buffer evicted oldest (6.0): samples = [12.0, 3.0, 9.0], avg = 8.0
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
            0,
        );
        // 15-min window with 5-min interval = 3 samples
        // Push sequence: 2, 4, 6 -> avg 4.0, then 8, 10, 12 -> avg 10.0
        for &kw in &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0] {
            state.update(kw, 300.0, 0.0, 0.0, 0, 0);
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
            0,
        );
        // Constant 1 kW for 1 hour (one step of 3600s)
        state.update(1.0, 3600.0, 0.10, 0.0, 0, 0);
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
            0,
        );
        // Constant -2 kW for 1 hour
        state.update(-2.0, 3600.0, 0.10, 0.05, 0, 0);
        assert!((state.cumulative_import_kwh).abs() < 1e-10);
        assert!((state.cumulative_export_kwh - 2.0).abs() < 1e-10);
        assert!((state.cumulative_export_credit_usd - 0.10).abs() < 1e-10);
    }

    #[test]
    fn billing_period_closes_at_month_end() {
        let start = make_dt(2025, 1, 1);
        let state = BillingState::new(start, BillingCycle::Monthly, 15, 3600, None, 0);
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
            0,
        );
        state.update(5.0, 3600.0, 0.10, 0.0, 0, 0);
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
            0,
        );
        // Simulate a high prior peak
        state.prior_peaks_kw.push_back(10.0);
        // Current peak is only 2 kW
        state.update(2.0, 3600.0, 0.0, 0.0, 0, 0);
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
            0,
        );
        state.prior_peaks_kw.push_back(5.0);
        // Push enough samples to fill the window with 10 kW
        // 15 min / 5 min = 3 samples
        for _ in 0..3 {
            state.update(10.0, 300.0, 0.0, 0.0, 0, 0);
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
            0,
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
    fn billing_ratchet_lookback_truncation() {
        let ratchet = RatchetConfig {
            lookback_months: 3,
            minimum_fraction: 0.85,
        };
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            Some(ratchet),
            0,
        );
        // Add 6 months of peaks: [5, 10, 15, 20, 25, 30]
        for peak in [5.0, 10.0, 15.0, 20.0, 25.0, 30.0] {
            state.prior_peaks_kw.push_back(peak);
        }
        // Lookback 3 → considers [20.0, 25.0, 30.0], max = 30.0
        // Effective = max(2.0, 0.85 * 30.0) = 25.5
        state.update(2.0, 3600.0, 0.0, 0.0, 0, 0);
        let effective = state.effective_peak_kw();
        assert!((effective - 25.5).abs() < 1e-10);
    }

    #[test]
    fn compute_period_end_jan31_to_feb28() {
        let start = make_dt(2025, 1, 31);
        let end = compute_period_end(start, BillingCycle::Monthly);
        // Feb 2025 has 28 days; day clamped to 28.
        assert_eq!(end.month(), 2);
        assert_eq!(end.day(), 28);
    }

    #[test]
    fn compute_period_end_jan29_leap_year() {
        let start = New_York
            .with_ymd_and_hms(2024, 1, 29, 0, 0, 0)
            .unwrap();
        let end = compute_period_end(start, BillingCycle::Monthly);
        // Feb 2024 has 29 days (leap year); day 29 fits.
        assert_eq!(end.month(), 2);
        assert_eq!(end.day(), 29);
    }

    #[test]
    fn compute_period_end_dec_to_jan() {
        let start = make_dt(2025, 12, 15);
        let end = compute_period_end(start, BillingCycle::Monthly);
        assert_eq!(end.year(), 2026);
        assert_eq!(end.month(), 1);
        assert_eq!(end.day(), 15);
    }

    #[test]
    fn billing_custom_cycle_period_end() {
        let start = make_dt(2025, 1, 1);
        let state = BillingState::new(start, BillingCycle::Custom(14), 15, 3600, None, 0);
        let expected_end = make_dt(2025, 1, 15);
        assert_eq!(state.period_end, expected_end);
    }

    #[test]
    fn compute_period_end_feb29_to_mar() {
        let start = New_York
            .with_ymd_and_hms(2024, 2, 29, 0, 0, 0)
            .unwrap();
        let end = compute_period_end(start, BillingCycle::Monthly);
        assert_eq!(end.month(), 3);
        assert_eq!(end.day(), 29);
    }

    #[test]
    fn billing_period_summary_net_bill() {
        let summary = BillingPeriodSummary::new(
            make_dt(2025, 1, 1),
            make_dt(2025, 2, 1),
            50.0,
            25.0,
            12.50,
            5.0,
            None,
            8.0,
            500.0,
            100.0,
        );
        let expected_net = summary.energy_charge_usd + summary.demand_charge_usd
            + summary.fixed_charge_usd - summary.export_credit_usd;
        assert!((summary.net_bill_usd - expected_net).abs() < 1e-10);
        assert!((summary.net_bill_usd - 82.5).abs() < 1e-10);
    }
}
