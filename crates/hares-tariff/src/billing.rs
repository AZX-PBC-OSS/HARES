use std::collections::VecDeque;

use chrono::{DateTime, Datelike, Duration, TimeZone};
use chrono_tz::Tz;
use hares_types::BillingCycle;

use crate::types::{RatchetConfig, TieredBlock};

struct DemandWindow {
    samples: Box<[f64]>,
    head: usize,
    count: usize,
    running_sum: f64,
    push_count: u64,
    #[cfg(feature = "observe")]
    partial_window_skips: u64,
}

impl DemandWindow {
    fn new(capacity: usize) -> Self {
        Self {
            samples: vec![0.0; capacity].into_boxed_slice(),
            head: 0,
            count: 0,
            running_sum: 0.0,
            push_count: 0,
            #[cfg(feature = "observe")]
            partial_window_skips: 0,
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

    fn average_full(&mut self) -> Option<f64> {
        if self.count == self.samples.len() {
            Some(self.running_sum / self.count as f64)
        } else {
            #[cfg(feature = "observe")]
            {
                self.partial_window_skips += 1;
            }
            None
        }
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.head = 0;
        self.count = 0;
        self.running_sum = 0.0;
        self.push_count = 0;
        #[cfg(feature = "observe")]
        {
            self.partial_window_skips = 0;
        }
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
    max_prior_periods: usize,
}

fn days_in_month(year: i32, month: u32) -> u32 {
    use chrono::NaiveDate;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next =
        NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("valid NaiveDate for day=1");
    let this = NaiveDate::from_ymd_opt(year, month, 1).expect("valid NaiveDate for day=1");
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
        max_lookback_months: u32,
        num_tou_periods: usize,
    ) -> Self {
        let capacity = if interval_seconds > 0 {
            (demand_window_minutes as usize * 60) / interval_seconds as usize
        } else {
            1
        };
        let capacity = capacity.max(1);
        let period_end = compute_period_end(period_start, billing_cycle);
        let max_prior_periods = if max_lookback_months == 0 {
            0
        } else {
            match billing_cycle {
                BillingCycle::Custom(days) if days > 0 => {
                    let lookback_days = max_lookback_months as usize * 31;
                    lookback_days.div_ceil(days as usize)
                }
                _ => max_lookback_months as usize,
            }
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
        if let Some(avg) = self.demand_window.average_full() {
            self.peak_demand_kw = self.peak_demand_kw.max(avg);
            if let Some(slot) = self.period_peak_demand_kw.get_mut(period_idx as usize) {
                *slot = slot.max(avg);
            }
            if demand_period_idx != period_idx {
                if let Some(slot) = self
                    .period_peak_demand_kw
                    .get_mut(demand_period_idx as usize)
                {
                    *slot = slot.max(avg);
                }
            }
        } else {
            tracing::debug!(
                target: "billing",
                push_count = self.demand_window.push_count,
                "skipping peak demand update: demand window not yet full"
            );
        }
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                self.demand_window.count <= self.demand_window.samples.len(),
                "demand window count {} exceeds capacity {}",
                self.demand_window.count,
                self.demand_window.samples.len()
            );
            debug_assert!(
                self.period_peak_demand_kw.len() == self.prior_period_peaks.len(),
                "period_peak_demand_kw len {} != prior_period_peaks len {}",
                self.period_peak_demand_kw.len(),
                self.prior_period_peaks.len()
            );
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

    /// Coincident peak with a caller-supplied ratchet (used per demand rate).
    pub fn effective_peak_with_ratchet(&self, ratchet: &Option<RatchetConfig>) -> f64 {
        apply_ratchet(self.peak_demand_kw, &self.prior_peaks_kw, ratchet)
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

/// Compute the total energy cost for a billing period using tiered block rates.
///
/// Walks the `TieredBlock` entries, selecting the first block whose season matches
/// `month`. Usage in each tier band is charged at that band's rate, producing a
/// correctly blended cost.
///
/// If no tiered block matches the month, returns `fallback_flat_cost` (the
/// step-accumulated energy cost from the billing state).
pub fn compute_tiered_energy_cost(
    import_kwh: f64,
    blocks: &[TieredBlock],
    month: u8,
    fallback_flat_cost: f64,
) -> f64 {
    let block = blocks.iter().find(|b| b.season.contains_month(month));
    let block = match block {
        Some(b) => b,
        None => return fallback_flat_cost,
    };

    let mut cost = 0.0;
    let mut remaining = import_kwh;
    let mut prev_threshold = 0.0;

    for (i, threshold) in block.thresholds_kwh.iter().enumerate() {
        let band_width = threshold - prev_threshold;
        let usage_in_band = remaining.min(band_width);
        cost += usage_in_band * block.rates_per_kwh[i];
        remaining -= usage_in_band;
        prev_threshold = *threshold;
        if remaining <= 0.0 {
            return cost;
        }
    }

    // All remaining usage is in the final (unbounded) tier.
    cost += remaining * block.rates_per_kwh[block.thresholds_kwh.len()];
    cost
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
        minimum_charge_excludes_export: bool,
        peak_demand_kw: f64,
        total_import_kwh: f64,
        total_export_kwh: f64,
    ) -> Self {
        let metered = energy_charge_usd + demand_charge_usd + fixed_charge_usd;
        let net_bill_usd = match minimum_charge {
            Some(min) if minimum_charge_excludes_export => {
                // Floor metered charges, then subtract export credit.
                metered.max(min) - export_credit_usd
            }
            Some(min) => {
                // Floor the net bill (after export credit).
                (metered - export_credit_usd).max(min)
            }
            None => metered - export_credit_usd,
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
            let lookback = prior_peaks.iter().rev().take(rc.lookback_months as usize);
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
    use hares_types::SeasonFilter;

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
        assert!(
            (w.average_full().unwrap() - 6.0).abs() < 1e-10,
            "first full window average should be 6.0"
        );
        w.push(12.0);
        // Ring buffer evicted oldest (6.0): samples = [12.0, 3.0, 9.0], avg = 8.0
        assert!(
            (w.average_full().unwrap() - 8.0).abs() < 1e-10,
            "rolling average after eviction should be 8.0"
        );
    }

    #[test]
    fn demand_window_peak_tracking() {
        let mut state =
            BillingState::new(make_dt(2025, 1, 1), BillingCycle::Monthly, 15, 300, 0, 0);
        // 15-min window with 5-min interval = 3 samples
        // Push sequence: 2, 4, 6 -> avg 4.0, then 8, 10, 12 -> avg 10.0
        for &kw in &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0] {
            state.update(kw, 300.0, 0.0, 0.0, 0, 0);
        }
        assert!((state.peak_demand_kw - 10.0).abs() < 1e-10);
    }

    #[test]
    fn billing_state_energy_accumulation() {
        let mut state =
            BillingState::new(make_dt(2025, 1, 1), BillingCycle::Monthly, 15, 3600, 0, 0);
        // Constant 1 kW for 1 hour (one step of 3600s)
        state.update(1.0, 3600.0, 0.10, 0.0, 0, 0);
        assert!((state.cumulative_import_kwh - 1.0).abs() < 1e-10);
        assert!((state.cumulative_export_kwh).abs() < 1e-10);
        assert!((state.cumulative_energy_cost_usd - 0.10).abs() < 1e-10);
    }

    #[test]
    fn billing_state_export_accumulation() {
        let mut state =
            BillingState::new(make_dt(2025, 1, 1), BillingCycle::Monthly, 15, 3600, 0, 0);
        // Constant -2 kW for 1 hour
        state.update(-2.0, 3600.0, 0.10, 0.05, 0, 0);
        assert!((state.cumulative_import_kwh).abs() < 1e-10);
        assert!((state.cumulative_export_kwh - 2.0).abs() < 1e-10);
        assert!((state.cumulative_export_credit_usd - 0.10).abs() < 1e-10);
    }

    #[test]
    fn billing_period_closes_at_month_end() {
        let start = make_dt(2025, 1, 1);
        let state = BillingState::new(start, BillingCycle::Monthly, 15, 3600, 0, 0);
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
            1, // need at least 1 month lookback to retain prior peak
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
            ratchet.lookback_months.into(),
            0,
        );
        // Simulate a high prior peak
        state.prior_peaks_kw.push_back(10.0);
        // Current peak is only 2 kW
        state.update(2.0, 3600.0, 0.0, 0.0, 0, 0);
        let effective = state.effective_peak_with_ratchet(&Some(ratchet));
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
            ratchet.lookback_months.into(),
            0,
        );
        state.prior_peaks_kw.push_back(5.0);
        for _ in 0..3 {
            state.update(10.0, 300.0, 0.0, 0.0, 0, 0);
        }
        let effective = state.effective_peak_with_ratchet(&Some(ratchet));
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
            12, // 12-month lookback
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
            ratchet.lookback_months.into(),
            0,
        );
        // Add 6 months of peaks: [5, 10, 15, 20, 25, 30]
        for peak in [5.0, 10.0, 15.0, 20.0, 25.0, 30.0] {
            state.prior_peaks_kw.push_back(peak);
        }
        // Lookback 3 → considers [20.0, 25.0, 30.0], max = 30.0
        // Effective = max(2.0, 0.85 * 30.0) = 25.5
        state.update(2.0, 3600.0, 0.0, 0.0, 0, 0);
        let effective = state.effective_peak_with_ratchet(&Some(ratchet));
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
        let start = New_York.with_ymd_and_hms(2024, 1, 29, 0, 0, 0).unwrap();
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
        let state = BillingState::new(start, BillingCycle::Custom(14), 15, 3600, 0, 0);
        let expected_end = make_dt(2025, 1, 15);
        assert_eq!(state.period_end, expected_end);
    }

    #[test]
    fn compute_period_end_feb29_to_mar() {
        let start = New_York.with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
        let end = compute_period_end(start, BillingCycle::Monthly);
        assert_eq!(end.month(), 3);
        assert_eq!(end.day(), 29);
    }

    // H1: Per-period ratchet with populated prior_period_peaks applies correctly.
    #[test]
    fn billing_per_period_ratchet_applied() {
        let ratchet = RatchetConfig {
            lookback_months: 3,
            minimum_fraction: 0.85,
        };
        // 3 TOU periods; we exercise period index 1.
        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Monthly,
            15,
            3600,
            ratchet.lookback_months.into(),
            3,
        );

        // Push three billing periods of history for period 1 via reset().
        // Period 1 peaks: 10.0, 15.0, 20.0 → max of last 3 = 20.0.
        for peak in [10.0_f64, 15.0, 20.0] {
            state.period_peak_demand_kw[1] = peak;
            let next = state.period_end;
            state.reset(next);
        }

        // Current period: push a single 5 kW reading (instant window, 1-sample average = 5.0).
        state.update(5.0, 3600.0, 0.0, 0.0, 1, 1);
        assert!((state.period_peak_demand_kw[1] - 5.0).abs() < 1e-10);

        // effective_peak_for_period(1) = max(5.0, 0.85 * 20.0) = max(5.0, 17.0) = 17.0
        let effective = state.effective_peak_for_period(1, &Some(ratchet));
        assert!(
            (effective - 17.0).abs() < 1e-10,
            "expected 17.0 (ratchet floor), got {effective}"
        );
    }

    // M4: BillingState with Custom(14) and lookback_months=11 caps prior_peaks_kw correctly.
    #[test]
    fn billing_custom_cycle_ratchet_max_prior_periods() {
        let ratchet = RatchetConfig {
            lookback_months: 11,
            minimum_fraction: 0.85,
        };
        // Custom(14)-day cycle with 11-month lookback.
        // max_prior_periods = ceil(11 * 31 / 14) = ceil(341 / 14) = ceil(24.36) = 25.
        let expected_cap: usize = (11usize * 31).div_ceil(14);

        let mut state = BillingState::new(
            make_dt(2025, 1, 1),
            BillingCycle::Custom(14),
            15,
            3600,
            ratchet.lookback_months.into(),
            0,
        );

        // Simulate 30 billing periods; prior_peaks_kw must not exceed the cap.
        for _ in 0..30 {
            state.peak_demand_kw = 5.0;
            let next = state.period_end;
            state.reset(next);
        }

        assert_eq!(
            state.prior_peaks_kw.len(),
            expected_cap,
            "prior_peaks_kw should be capped at {expected_cap}, got {}",
            state.prior_peaks_kw.len()
        );
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
            true,
            8.0,
            500.0,
            100.0,
        );
        let expected_net =
            summary.energy_charge_usd + summary.demand_charge_usd + summary.fixed_charge_usd
                - summary.export_credit_usd;
        assert!((summary.net_bill_usd - expected_net).abs() < 1e-10);
        assert!((summary.net_bill_usd - 82.5).abs() < 1e-10);
    }

    #[test]
    fn compute_tiered_energy_cost_blended_across_boundary() {
        // Two tiers: 0-500 kWh at $0.10, 500+ kWh at $0.20
        let blocks = vec![TieredBlock {
            season: SeasonFilter::Summer,
            thresholds_kwh: vec![500.0],
            rates_per_kwh: vec![0.10, 0.20],
        }];

        // 750 kWh in July (summer, month 7):
        //   500 * 0.10 = $50.00
        //   250 * 0.20 = $50.00
        //   total = $100.00
        let cost = compute_tiered_energy_cost(750.0, &blocks, 7, 999.0);
        assert!((cost - 100.0).abs() < 1e-10, "expected $100.00, got {cost}");
    }

    #[test]
    fn compute_tiered_energy_cost_three_tiers() {
        // Three tiers: 0-300 at $0.08, 300-800 at $0.12, 800+ at $0.25
        let blocks = vec![TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![300.0, 800.0],
            rates_per_kwh: vec![0.08, 0.12, 0.25],
        }];

        // 1000 kWh:
        //   300 * 0.08 = $24.00
        //   500 * 0.12 = $60.00
        //   200 * 0.25 = $50.00
        //   total = $134.00
        let cost = compute_tiered_energy_cost(1000.0, &blocks, 1, 999.0);
        assert!((cost - 134.0).abs() < 1e-10, "expected $134.00, got {cost}");
    }

    #[test]
    fn compute_tiered_energy_cost_within_first_tier() {
        let blocks = vec![TieredBlock {
            season: SeasonFilter::All,
            thresholds_kwh: vec![500.0],
            rates_per_kwh: vec![0.10, 0.20],
        }];

        // 200 kWh: entirely in first tier = 200 * 0.10 = $20.00
        let cost = compute_tiered_energy_cost(200.0, &blocks, 1, 999.0);
        assert!((cost - 20.0).abs() < 1e-10, "expected $20.00, got {cost}");
    }

    #[test]
    fn compute_tiered_energy_cost_no_matching_season_returns_fallback() {
        // Summer-only block, but month is January (winter).
        let blocks = vec![TieredBlock {
            season: SeasonFilter::Summer,
            thresholds_kwh: vec![500.0],
            rates_per_kwh: vec![0.10, 0.20],
        }];

        let cost = compute_tiered_energy_cost(750.0, &blocks, 1, 42.0);
        assert!(
            (cost - 42.0).abs() < 1e-10,
            "expected fallback $42.00, got {cost}"
        );
    }

    #[test]
    fn minimum_charge_excludes_export_true() {
        // metered = 20 + 5 + 10 = $35; min = $50; export = $30
        // excludes_export=true: net = max(35, 50) - 30 = 50 - 30 = $20
        let summary = BillingPeriodSummary::new(
            make_dt(2025, 1, 1),
            make_dt(2025, 2, 1),
            20.0, // energy
            5.0,  // demand
            10.0, // fixed
            30.0, // export credit
            Some(50.0),
            true, // minimum_charge_excludes_export
            5.0,
            500.0,
            400.0,
        );
        assert!(
            (summary.net_bill_usd - 20.0).abs() < 1e-10,
            "expected $20.00 (floor metered then subtract export), got {}",
            summary.net_bill_usd
        );
    }

    #[test]
    fn minimum_charge_excludes_export_false() {
        // metered = 20 + 5 + 10 = $35; export = $30; raw_net = 35 - 30 = $5; min = $50
        // excludes_export=false: net = max(5, 50) = $50
        let summary = BillingPeriodSummary::new(
            make_dt(2025, 1, 1),
            make_dt(2025, 2, 1),
            20.0,
            5.0,
            10.0,
            30.0,
            Some(50.0),
            false,
            5.0,
            500.0,
            400.0,
        );
        assert!(
            (summary.net_bill_usd - 50.0).abs() < 1e-10,
            "expected $50.00 (floor applied to net bill), got {}",
            summary.net_bill_usd
        );
    }

    #[test]
    fn demand_window_average_full_returns_none_when_partial() {
        let mut w = DemandWindow::new(3);
        assert!(w.average_full().is_none(), "empty window: count=0, cap=3");
        w.push(10.0);
        assert!(w.average_full().is_none(), "count=1 < cap=3");
        w.push(10.0);
        assert!(w.average_full().is_none(), "count=2 < cap=3");
        w.push(10.0);
        let avg = w.average_full().expect("full window should return Some");
        assert!((avg - 10.0).abs() < 1e-10, "expected 10.0, got {avg}");
    }

    #[test]
    fn demand_window_average_full_after_reset_requires_refill() {
        let mut w = DemandWindow::new(3);
        // Fill the window.
        for _ in 0..3 {
            w.push(5.0);
        }
        assert!(w.average_full().is_some(), "window should be full");
        // Reset clears all state.
        w.reset();
        assert!(
            w.average_full().is_none(),
            "after reset, window should be empty"
        );
        // Push one sample — still not full.
        w.push(8.0);
        assert!(w.average_full().is_none(), "count=1 < cap=3 after reset");
        // Fill to capacity.
        w.push(8.0);
        w.push(8.0);
        let avg = w.average_full().expect("window should be full");
        assert!((avg - 8.0).abs() < 1e-10);
    }

    #[test]
    fn billing_state_post_reset_spike_does_not_set_peak_demand() {
        // 15-minute demand window with 5-minute intervals → capacity = 3
        let mut state =
            BillingState::new(make_dt(2025, 1, 1), BillingCycle::Monthly, 15, 300, 0, 0);
        // Push a spike in the first post-reset step (count=1, window not full).
        state.update(100.0, 300.0, 0.0, 0.0, 0, 0);
        assert!(
            state.peak_demand_kw.abs() < 1e-10,
            "spike in partial window should not set peak; got {}",
            state.peak_demand_kw
        );
        // Push two more normal readings to fill the window.
        state.update(1.0, 300.0, 0.0, 0.0, 0, 0);
        state.update(1.0, 300.0, 0.0, 0.0, 0, 0);
        // Window now contains [100, 1, 1] → avg = 34.0. The spike is part of the
        // full-window average, so peak reflects the 15-min sliding window average.
        // Window [100, 1, 1] → (100 + 1 + 1) / 3 = 34.0.
        let peak = state.peak_demand_kw;
        assert!(
            (peak - 34.0).abs() < 1e-10,
            "after window fills, peak should be 34.0; got {peak}"
        );
    }

    #[test]
    fn billing_state_sustained_peak_across_full_window_is_captured() {
        // 15-minute demand window with 5-minute intervals → capacity = 3
        let mut state =
            BillingState::new(make_dt(2025, 1, 1), BillingCycle::Monthly, 15, 300, 0, 0);
        // Push three reads of 10 kW → window fills, avg = 10, peak = 10.
        for _ in 0..3 {
            state.update(10.0, 300.0, 0.0, 0.0, 0, 0);
        }
        assert!(
            (state.peak_demand_kw - 10.0).abs() < 1e-10,
            "expected peak 10.0, got {}",
            state.peak_demand_kw
        );
        // Push three more reads of 20 kW → window fills with [20,20,20], peak = 20.
        for _ in 0..3 {
            state.update(20.0, 300.0, 0.0, 0.0, 0, 0);
        }
        assert!(
            (state.peak_demand_kw - 20.0).abs() < 1e-10,
            "expected peak 20.0, got {}",
            state.peak_demand_kw
        );
    }
}
