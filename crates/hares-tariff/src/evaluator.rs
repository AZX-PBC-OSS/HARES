use chrono::{DateTime, Datelike, Duration, Timelike};
use chrono_tz::Tz;
use hares_types::{HaresError, season_contains_month};

use crate::billing::{BillingPeriodSummary, BillingState, compute_tiered_energy_cost};
use crate::types::{ElectricTariff, ExportMode};

pub struct TariffEvaluator {
    tariff: ElectricTariff,
    price_array: Vec<f64>,
    export_array: Vec<f64>,
    /// Precomputed EV energy rate per timestep ($/kWh).
    /// Zero when no EV sub-tariff is configured.
    ev_price_array: Vec<f64>,
    /// Precomputed hour-of-year for each timestep (0-8759 in local civil time).
    hour_of_year_array: Vec<usize>,
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
    finalized: bool,
    /// Observer: number of timesteps the RTP price path was taken.
    #[cfg(feature = "observe")]
    pub rtp_price_used: u64,
    /// Observer: number of timesteps a CPP event rate was applied.
    #[cfg(feature = "observe")]
    pub cpp_event_triggered: u64,
    /// Observer: number of timesteps a CPP event was scheduled but the
    /// annual event-hour limit was already reached.
    #[cfg(feature = "observe")]
    pub cpp_event_limit_hit: u64,
    /// Observer: cumulative kWh billed under the EV-specific rate.
    #[cfg(feature = "observe")]
    pub ev_rate_applied_kwh: f64,
    /// Observer: number of timesteps the hourly export price schedule was used.
    #[cfg(feature = "observe")]
    pub export_price_schedule_used: u64,
    /// Observer: number of timesteps the hourly schedule was defined but
    /// the hour-of-year was out of bounds (guard against missing hours).
    #[cfg(feature = "observe")]
    pub tou_credits_fallback: u64,
    /// Observer: minimum export price seen during the billing period ($/kWh).
    #[cfg(feature = "observe")]
    pub export_price_min: f64,
    /// Observer: maximum export price seen during the billing period ($/kWh).
    #[cfg(feature = "observe")]
    pub export_price_max: f64,
    /// Observer: sum of export prices seen (for mean computation).
    #[cfg(feature = "observe")]
    pub export_price_sum: f64,
    /// Observer: number of export price samples (for mean computation).
    #[cfg(feature = "observe")]
    pub export_price_count: u64,
    /// Observer: whether a SeasonalSplit was supplied at construction time.
    #[cfg(feature = "observe")]
    pub seasonal_split_used: bool,
    /// Observer: summer months detected from the SeasonalSplit (1-indexed),
    /// captured once at tariff load time.
    #[cfg(feature = "observe")]
    pub seasonal_split_summer_months: Vec<u32>,
    /// Observer: whether shoulder months are configured on the SeasonalSplit.
    #[cfg(feature = "observe")]
    pub seasonal_split_has_shoulder: bool,
    /// Observer: shoulder months detected from the SeasonalSplit (1-indexed),
    /// captured once at tariff load time.
    #[cfg(feature = "observe")]
    pub shoulder_months: Vec<u32>,
}

impl TariffEvaluator {
    pub fn new(
        tariff: ElectricTariff,
        simulation_start: DateTime<Tz>,
        simulation_end: DateTime<Tz>,
        interval_seconds: u32,
    ) -> Result<Self, HaresError> {
        if interval_seconds == 0 {
            return Err(HaresError::Tariff("interval_seconds must be > 0".into()));
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
        if num_steps == 0 {
            return Err(HaresError::Tariff(
                "simulation duration is shorter than one interval -- no steps to simulate".into(),
            ));
        }
        let mut price_array = Vec::with_capacity(num_steps);
        let mut export_array = Vec::with_capacity(num_steps);
        let mut ev_price_array = Vec::with_capacity(num_steps);
        let mut hour_of_year_array = Vec::with_capacity(num_steps);
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

        let seasonal_split = tariff.seasonal_split.as_ref();

        #[cfg(feature = "observe")]
        let seasonal_split_used = seasonal_split.is_some();
        #[cfg(feature = "observe")]
        let seasonal_split_summer_months: Vec<u32> = seasonal_split
            .map(|ss| (1..=12u32).filter(|&m| ss.is_summer(m as u8)).collect())
            .unwrap_or_default();
        #[cfg(feature = "observe")]
        let seasonal_split_has_shoulder = seasonal_split.is_some_and(|ss| ss.has_shoulder());
        #[cfg(feature = "observe")]
        let shoulder_months: Vec<u32> = seasonal_split
            .map(|ss| (1..=12u32).filter(|&m| ss.is_shoulder(m as u8)).collect())
            .unwrap_or_default();

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if let Some(split) = seasonal_split {
            if split.has_seasonal() {
                for m in 1..=12u8 {
                    let split_summer = split.is_summer(m);
                    let default_summer = (6..=9).contains(&m);
                    if split_summer != default_summer {
                        tracing::debug!(
                            month = m,
                            split_summer,
                            default_summer,
                            summer_start = split.summer_start_month,
                            summer_end = split.summer_end_month,
                            "SeasonalSplit reclassifies month relative to June–September default"
                        );
                    }
                }
            }
            if split.has_shoulder() {
                for m in 1..=12u8 {
                    if split.is_summer(m) && split.is_shoulder(m) {
                        debug_assert!(
                            false,
                            "SeasonalSplit shoulder range overlaps with summer range at month {m}"
                        );
                        tracing::error!(
                            month = m,
                            summer_start = split.summer_start_month,
                            summer_end = split.summer_end_month,
                            shoulder_start = split.shoulder_start,
                            shoulder_end = split.shoulder_end,
                            "SeasonalSplit shoulder range overlaps with summer range; shoulder takes precedence"
                        );
                    }
                }
            }
        }

        for i in 0..num_steps {
            let ts = simulation_start + Duration::seconds(i as i64 * interval_seconds as i64);
            let civil = ts.with_timezone(&timezone);
            let month = civil.month() as u8;
            let weekday = civil.weekday();
            let minute_of_day = civil.hour() as u16 * 60 + civil.minute() as u16;
            let hour_of_year = (civil.ordinal0() as usize * 24 + civil.hour() as usize) % 8760;

            let mut matched_period: Option<&str> = None;
            for period in &tariff.tou_schedule {
                if !season_contains_month(period.season, seasonal_split, month) {
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
                    let rate = if let Some(ref rtp) = tariff.rtp_schedule {
                        let hoy = hour_of_year;
                        rtp.get(hoy).copied().unwrap_or(0.0)
                    } else {
                        tariff
                            .energy_rates
                            .iter()
                            .find(|er| {
                                er.period_name == name
                                    && season_contains_month(er.season, seasonal_split, month)
                            })
                            .map(|er| er.rate_per_kwh)
                            .unwrap_or(0.0)
                    };
                    (rate, intern(name))
                }
                None => {
                    let price = if let Some(ref rtp) = tariff.rtp_schedule {
                        rtp.get(hour_of_year).copied().unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    (price, 0)
                }
            };

            // Precompute EV energy rate for this timestep.
            let ev_price = if let Some(ref ev_name) = tariff.ev_tou_period_name {
                tariff
                    .energy_rates
                    .iter()
                    .find(|er| {
                        er.period_name == *ev_name
                            && season_contains_month(er.season, seasonal_split, month)
                    })
                    .map(|er| er.rate_per_kwh)
                    .unwrap_or(0.0)
            } else {
                0.0
            };

            let export_price = match &tariff.export_rate.mode {
                ExportMode::NetMetering => import_price,
                ExportMode::FlatRate(r) => *r,
                ExportMode::HourlySchedule(schedule) => {
                    schedule.get(hour_of_year).copied().unwrap_or(0.0)
                }
                ExportMode::NetBilling => {
                    if let Some(name) = matched_period {
                        tariff
                            .export_rate
                            .tou_credits
                            .iter()
                            .find(|er| {
                                er.period_name == name
                                    && season_contains_month(er.season, seasonal_split, month)
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
                    if !season_contains_month(period.season, seasonal_split, month) {
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
            ev_price_array.push(ev_price);
            hour_of_year_array.push(hour_of_year);
            period_indices.push(period_idx);
            demand_period_indices.push(demand_idx);
            months.push(month);
        }

        // Compute max lookback across all demand rates' ratchets so BillingState
        // retains enough history for any per-rate ratchet to work.
        let max_lookback_months = tariff
            .demand_rates
            .iter()
            .filter_map(|dr| dr.ratchet.as_ref())
            .map(|r| r.lookback_months)
            .max()
            .unwrap_or(0);

        let demand_window_minutes = tariff.demand_window_minutes;
        // When the demand window is shorter than the simulation interval,
        // it effectively becomes a 1-sample instantaneous peak. This is
        // physically correct (the interval IS the averaging window). Only
        // warn when the window is longer than the interval but not evenly
        // divisible -- that silently truncates the averaging window.
        let window_seconds = demand_window_minutes as u64 * 60;
        // Why: clippy::manual_is_multiple_of fires on `a % b != 0` even when the
        // semantics are "is not evenly divisible" — the guard checks
        // non-divisibility, not divisibility, and `is_multiple_of()` reads
        // awkwardly when negated.
        #[allow(clippy::manual_is_multiple_of)]
        if window_seconds > interval_seconds as u64 && window_seconds % interval_seconds as u64 != 0
        {
            return Err(HaresError::Tariff(format!(
                "demand_window_minutes ({demand_window_minutes}) must be evenly \
                 divisible by the simulation interval ({interval_seconds}s) \
                 when the window exceeds the interval",
            )));
        }
        let billing_state = BillingState::new(
            simulation_start,
            tariff.billing_cycle,
            demand_window_minutes,
            interval_seconds,
            max_lookback_months.into(),
            period_name_table.len(),
        );

        debug_assert_eq!(price_array.len(), export_array.len());
        debug_assert_eq!(price_array.len(), ev_price_array.len());
        debug_assert_eq!(price_array.len(), hour_of_year_array.len());
        debug_assert_eq!(price_array.len(), period_indices.len());
        debug_assert_eq!(price_array.len(), demand_period_indices.len());
        debug_assert_eq!(price_array.len(), months.len());

        Ok(Self {
            tariff,
            price_array,
            export_array,
            ev_price_array,
            hour_of_year_array,
            period_indices,
            demand_period_indices,
            months,
            period_name_table,
            interval_seconds,
            simulation_start,
            step_index: 0,
            billing_state,
            finished: false,
            finalized: false,
            #[cfg(feature = "observe")]
            rtp_price_used: 0,
            #[cfg(feature = "observe")]
            cpp_event_triggered: 0,
            #[cfg(feature = "observe")]
            cpp_event_limit_hit: 0,
            #[cfg(feature = "observe")]
            ev_rate_applied_kwh: 0.0,
            #[cfg(feature = "observe")]
            export_price_schedule_used: 0,
            #[cfg(feature = "observe")]
            tou_credits_fallback: 0,
            #[cfg(feature = "observe")]
            export_price_min: f64::INFINITY,
            #[cfg(feature = "observe")]
            export_price_max: f64::NEG_INFINITY,
            #[cfg(feature = "observe")]
            export_price_sum: 0.0,
            #[cfg(feature = "observe")]
            export_price_count: 0,
            #[cfg(feature = "observe")]
            seasonal_split_used,
            #[cfg(feature = "observe")]
            seasonal_split_summer_months,
            #[cfg(feature = "observe")]
            seasonal_split_has_shoulder,
            #[cfg(feature = "observe")]
            shoulder_months,
        })
    }

    /// Clamped index: returns the last valid index when step_index exceeds bounds.
    fn clamped_index(&self) -> usize {
        debug_assert!(
            !self.price_array.is_empty(),
            "price_array must be non-empty"
        );
        self.step_index
            .min(self.price_array.len().saturating_sub(1))
    }

    pub fn current_price(&self) -> f64 {
        self.price_array[self.clamped_index()]
    }

    pub fn current_export_price(&self) -> f64 {
        self.export_array[self.clamped_index()]
    }

    pub fn current_period_name(&self) -> &str {
        let idx = self.period_indices[self.clamped_index()] as usize;
        &self.period_name_table[idx]
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Advance to the next timestep. Returns `false` if already at the last step.
    pub fn advance(&mut self) -> bool {
        if self.step_index + 1 < self.price_array.len() {
            self.step_index += 1;
            true
        } else {
            self.finished = true;
            false
        }
    }

    pub fn tier_rate_at(&self, cumulative_kwh: f64) -> f64 {
        let ci = self.clamped_index();
        let month = self.months[ci];

        for block in &self.tariff.tiered_rates {
            if !season_contains_month(block.season, self.tariff.seasonal_split.as_ref(), month) {
                continue;
            }
            for (i, threshold) in block.thresholds_kwh.iter().enumerate() {
                if cumulative_kwh < *threshold {
                    return block.rates_per_kwh[i];
                }
            }
            return block.rates_per_kwh[block.thresholds_kwh.len()];
        }

        // No tiered block for current season -- return the current step's energy price.
        self.price_array[ci]
    }

    /// Returns a subslice of the price array, or `None` if indices are out of bounds.
    pub fn price_slice(&self, start_idx: usize, end_idx: usize) -> Option<&[f64]> {
        self.price_array.get(start_idx..end_idx)
    }

    pub fn step(
        &mut self,
        net_power_kw: f64,
        ev_power_kw: f64,
        dt_seconds: f64,
        current_time: DateTime<Tz>,
    ) -> Option<BillingPeriodSummary> {
        if self.finished {
            return None;
        }
        debug_assert!(
            self.step_index < self.price_array.len(),
            "step_index {} out of bounds (len {})",
            self.step_index,
            self.price_array.len()
        );
        let mut import_price = self.current_price();
        let export_price = self.current_export_price();

        // Resolve effective import price: CPP event override.
        let ci = self.clamped_index();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if let Some(ref cpp) = self.tariff.cpp_config {
                for (i, &val) in cpp.event_schedule.iter().enumerate() {
                    debug_assert!(val == 0 || i < 8760, "cpp event_schedule index {i} >= 8760");
                }
            }
        }
        let _cpp_active = if let Some(ref cpp) = self.tariff.cpp_config {
            let hoy = self.hour_of_year_array[ci];
            let is_event = cpp.event_schedule.get(hoy).copied().unwrap_or(0) != 0;
            if is_event {
                if self
                    .billing_state
                    .try_cpp_event(hoy, true, cpp.event_count_limit)
                {
                    import_price = cpp.event_rate_per_kwh;
                    #[cfg(feature = "observe")]
                    {
                        self.cpp_event_triggered += 1;
                    }
                    true
                } else {
                    #[cfg(feature = "observe")]
                    {
                        self.cpp_event_limit_hit += 1;
                    }
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        // Compute EV import kWh and effective blended import price.
        let ev_import_kwh = if self.tariff.ev_tou_period_name.is_some() && ev_power_kw > 0.0 {
            let ev_import_kw = ev_power_kw.min(net_power_kw).max(0.0);
            ev_import_kw * dt_seconds / 3600.0
        } else {
            0.0
        };

        let import_kwh = net_power_kw.max(0.0) * dt_seconds / 3600.0;
        let non_ev_import_kwh = import_kwh - ev_import_kwh;

        // Compute blended import price: non-EV at standard (or CPP/RTP) rate,
        // EV at EV-specific rate. This preserves total energy cost correctness
        // while allowing the existing `update()` cost accumulation to work unchanged.
        let ev_import_price = self.ev_price_array[ci];
        let effective_import_price = if import_kwh > 0.0 {
            (non_ev_import_kwh * import_price + ev_import_kwh * ev_import_price) / import_kwh
        } else {
            import_price
        };

        #[cfg(feature = "observe")]
        {
            if self.tariff.rtp_schedule.is_some() {
                self.rtp_price_used += 1;
            }
            self.ev_rate_applied_kwh += ev_import_kwh;
            if matches!(&self.tariff.export_rate.mode, ExportMode::HourlySchedule(_)) {
                self.export_price_schedule_used += 1;
                let hoy = self.hour_of_year_array[ci];
                if let ExportMode::HourlySchedule(ref schedule) = self.tariff.export_rate.mode {
                    if hoy < schedule.len() {
                        let price = schedule[hoy];
                        self.export_price_min = self.export_price_min.min(price);
                        self.export_price_max = self.export_price_max.max(price);
                        self.export_price_sum += price;
                        self.export_price_count += 1;
                    } else {
                        self.tou_credits_fallback += 1;
                    }
                }
            }
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if let Some(ref cpp) = self.tariff.cpp_config {
                debug_assert!(
                    self.billing_state.running_cpp_event_hours() <= cpp.event_count_limit,
                    "CPP event count {} exceeds annual limit {}",
                    self.billing_state.running_cpp_event_hours(),
                    cpp.event_count_limit
                );
            }
            if let Some(ref rtp) = self.tariff.rtp_schedule {
                let hoy = self.hour_of_year_array[ci];
                debug_assert!(
                    hoy < rtp.len(),
                    "hour_of_year {hoy} out of bounds for rtp_schedule (len {})",
                    rtp.len()
                );
                debug_assert!(
                    rtp[hoy] >= 0.0 && rtp[hoy].is_finite(),
                    "rtp_schedule[{hoy}] = {} is invalid",
                    rtp[hoy]
                );
            }
            if let ExportMode::HourlySchedule(ref schedule) = self.tariff.export_rate.mode {
                debug_assert_eq!(
                    schedule.len(),
                    8760,
                    "HourlySchedule must have exactly 8760 entries"
                );
                let hoy = self.hour_of_year_array[ci];
                debug_assert!(
                    hoy < schedule.len(),
                    "hour_of_year {hoy} out of bounds for HourlySchedule (len {})",
                    schedule.len()
                );
                debug_assert!(
                    schedule[hoy] >= 0.0 && schedule[hoy].is_finite(),
                    "HourlySchedule[{hoy}] = {} is invalid",
                    schedule[hoy]
                );
            }
        }

        let period_idx = self.period_indices[self.step_index];
        let demand_period_idx = self.demand_period_indices[self.step_index];
        self.billing_state.update(
            net_power_kw,
            dt_seconds,
            effective_import_price,
            export_price,
            period_idx,
            demand_period_idx,
            ev_import_kwh,
        );

        let result = if current_time >= self.billing_state.period_end {
            let month = self.billing_state.period_start.month() as u8;

            let demand_charge = self.compute_demand_charge(month);

            let days_in_period =
                (self.billing_state.period_end - self.billing_state.period_start).num_days() as f64;
            let fixed_charge = self.tariff.fixed_charges.monthly_usd
                + self.tariff.fixed_charges.daily_usd * days_in_period;

            let energy_charge = compute_tiered_energy_cost(
                self.billing_state.cumulative_import_kwh,
                &self.tariff.tiered_rates,
                month,
                self.tariff.seasonal_split.as_ref(),
                self.billing_state.cumulative_energy_cost_usd,
            );
            let export_credit = self.billing_state.cumulative_export_credit_usd;

            let summary = BillingPeriodSummary::new(
                self.billing_state.period_start,
                self.billing_state.period_end,
                energy_charge,
                demand_charge,
                fixed_charge,
                export_credit,
                self.tariff.minimum_charge,
                self.tariff.minimum_charge_excludes_export,
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
        self.tariff
            .demand_rates
            .iter()
            .filter(|dr| season_contains_month(dr.season, self.tariff.seasonal_split.as_ref(), month))
            .map(|dr| {
                let peak = match &dr.period_name {
                    // Coincident demand: use each rate's own ratchet, not a global one.
                    None => self
                        .billing_state
                        .effective_peak_with_ratchet(&dr.ratchet),
                    Some(name) => {
                        let idx = self
                            .period_name_table
                            .iter()
                            .position(|n| n == name)
                            .expect("demand period name not in table -- validate() should have caught this")
                            as u16;
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
    ///
    /// `sim_end` is the actual simulation end time, used to prorate fixed
    /// charges for partial periods. If the simulation ends mid-month, only
    /// the elapsed days are charged.
    ///
    /// Returns `None` on second call (idempotent -- `finalized` flag prevents double-billing).
    /// Returns `Some` even with zero metered load, as fixed charges may apply.
    pub fn finalize(&mut self, sim_end: DateTime<Tz>) -> Option<BillingPeriodSummary> {
        if self.finalized {
            return None;
        }
        self.finalized = true;
        self.finished = true;

        let month = self.billing_state.period_start().month() as u8;
        let demand_charge = self.compute_demand_charge(month);

        // Prorate fixed charges: use actual elapsed days, not the full
        // scheduled period length. Clamp to period end in case sim_end
        // exceeds the billing period boundary.
        let actual_end = sim_end.min(self.billing_state.period_end());
        let elapsed_days = (actual_end - self.billing_state.period_start())
            .num_days()
            .max(0) as f64;
        let fixed_charge = self.tariff.fixed_charges.monthly_usd
            + self.tariff.fixed_charges.daily_usd * elapsed_days;

        let energy_charge = compute_tiered_energy_cost(
            self.billing_state.cumulative_import_kwh(),
            &self.tariff.tiered_rates,
            month,
            self.tariff.seasonal_split.as_ref(),
            self.billing_state.cumulative_energy_cost_usd(),
        );
        let export_credit = self.billing_state.cumulative_export_credit_usd();
        let peak = self.billing_state.peak_demand_kw();
        let import = self.billing_state.cumulative_import_kwh();
        let export = self.billing_state.cumulative_export_kwh();
        let start = self.billing_state.period_start();
        let end = actual_end;

        Some(BillingPeriodSummary::new(
            start,
            end,
            energy_charge,
            demand_charge,
            fixed_charge,
            export_credit,
            self.tariff.minimum_charge,
            self.tariff.minimum_charge_excludes_export,
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
    use hares_types::{
        BillingCycle, DayFilter, SeasonFilter, SeasonalSplit, TimeWindow, TouPeriod,
    };

    use crate::types::{CppConfig, EnergyRate, ExportRate, FixedCharges, TieredBlock};

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
        let ev = make_evaluator(
            test_tariff(),
            make_start(2025, 1, 1),
            make_start(2026, 1, 1),
            3600,
        );
        assert_eq!(ev.total_steps(), 8760);
    }

    #[test]
    fn evaluator_15min_array_length() {
        let ev = make_evaluator(
            test_tariff(),
            make_start(2025, 1, 1),
            make_start(2026, 1, 1),
            900,
        );
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
    fn evaluator_tier_rate_at_first_tier() {
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(test_tariff(), start, start + Duration::hours(1), 3600);
        assert_eq!(ev.tier_rate_at(0.0), 0.10);
        assert_eq!(ev.tier_rate_at(499.9), 0.10);
    }

    #[test]
    fn evaluator_tier_rate_at_second_tier() {
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev = make_evaluator(test_tariff(), start, start + Duration::hours(1), 3600);
        assert_eq!(ev.tier_rate_at(500.0), 0.20);
        assert_eq!(ev.tier_rate_at(1000.0), 0.20);
    }

    #[test]
    fn evaluator_price_slice() {
        let ev = make_evaluator(
            test_tariff(),
            make_start(2025, 1, 1),
            make_start(2025, 1, 2),
            3600,
        );
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
        let ev = make_evaluator(
            flat_tariff,
            make_start(2025, 1, 1),
            make_start(2026, 1, 1),
            3600,
        );
        assert_eq!(ev.total_steps(), 8760);
        for price in &ev.price_array {
            assert_eq!(*price, 0.12);
        }
    }

    #[test]
    fn evaluator_no_match_returns_zero() {
        // Tariff with only a summer on-peak period -- winter timestamps have no match.
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
        // January weekday -- no matching period.
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
            if let Some(s) = ev.step(power_fn(i), 0.0, interval as f64, step_end) {
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

        assert_eq!(
            summaries.len(),
            2,
            "expected exactly 2 billing period closes"
        );

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

        let summary = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
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

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");

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

        let summary = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
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

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
        let expected =
            s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd - s.export_credit_usd;
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
    fn evaluator_tier_rate_at_no_tiers_returns_price() {
        let start = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let tariff = flat_tariff(0.12);
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.tier_rate_at(0.0) - 0.12).abs() < 1e-10,
            "with no tiers, tier_rate_at should return current_price (0.12), got {}",
            ev.tier_rate_at(0.0)
        );
        assert!(
            (ev.tier_rate_at(999.0) - 0.12).abs() < 1e-10,
            "with no tiers, tier_rate_at should return current_price regardless of kwh"
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

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
        let raw =
            s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd - s.export_credit_usd;
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

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
        let raw =
            s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd - s.export_credit_usd;
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

        // Monday January 6, 2025 -- a weekday.
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

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
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
        ev.step(5.0, 0.0, interval as f64, step_end);

        let first = ev.finalize(end);
        assert!(first.is_some(), "first finalize() should return Some");

        let second = ev.finalize(end);
        assert!(
            second.is_none(),
            "second finalize() should return None after billing state was reset"
        );
    }

    #[test]
    fn evaluator_finalize_partial_period_prorates_fixed_charge() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1); // full month
        let interval = 3600u32;
        let daily_usd = 1.0;
        let mut tariff = flat_tariff(0.12);
        tariff.fixed_charges = FixedCharges {
            monthly_usd: 0.0,
            daily_usd,
        };
        let mut ev = make_evaluator(tariff, start, end, interval);

        // Step through 15 days only.
        let sim_end = make_start(2025, 1, 16);
        let steps_15d = 15 * 24;
        for i in 0..steps_15d {
            let t = start + Duration::seconds((i as i64 + 1) * interval as i64);
            ev.step(1.0, 0.0, interval as f64, t);
        }

        let summary = ev.finalize(sim_end).expect("should have charges");
        // 15 elapsed days × $1/day = $15
        assert!(
            (summary.fixed_charge_usd - 15.0).abs() < 1e-10,
            "partial-period fixed charge should be $15 (15 days × $1/day), got {}",
            summary.fixed_charge_usd
        );
    }

    #[test]
    fn evaluator_finalize_full_period_unchanged() {
        // Simulate 31 days but DON'T trigger a billing period close (the billing
        // period is monthly starting Jan 1, ending Feb 1 = 31 days). We step
        // through all hours but stop just before the period_end to avoid auto-close.
        // Then finalize with sim_end == period_end to get the full-period charge.
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1); // 31 days
        let interval = 3600u32;
        let daily_usd = 1.0;
        let mut tariff = flat_tariff(0.12);
        tariff.fixed_charges = FixedCharges {
            monthly_usd: 0.0,
            daily_usd,
        };
        let mut ev = make_evaluator(tariff, start, end, interval);

        // Step all hours except the last one (which would trigger period close).
        let total = ev.total_steps();
        for i in 0..(total - 1) {
            let t = start + Duration::seconds((i as i64 + 1) * interval as i64);
            ev.step(1.0, 0.0, interval as f64, t);
        }

        let summary = ev.finalize(end).expect("should have charges");
        // Full period: 31 days × $1/day = $31
        assert!(
            (summary.fixed_charge_usd - 31.0).abs() < 1e-10,
            "full-period fixed charge should be $31, got {}",
            summary.fixed_charge_usd
        );
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
        let _ = ev.step(1.0, 0.0, interval as f64, step_end);

        // Any subsequent call must return None -- the evaluator is finished.
        let result = ev.step(1.0, 0.0, interval as f64, step_end + Duration::hours(1));
        assert!(
            result.is_none(),
            "step() after simulation end should return None"
        );
    }

    #[test]
    fn accessor_returns_last_value_after_advance_exhaustion() {
        let start = make_start(2025, 1, 1);
        let mut ev = make_evaluator(flat_tariff(0.12), start, start + Duration::hours(3), 3600);
        assert_eq!(ev.total_steps(), 3);

        // Advance to the end.
        assert!(ev.advance());
        assert!(ev.advance());
        assert!(!ev.advance()); // exhausted

        // Accessors must not panic -- they return the last valid value.
        assert_eq!(ev.current_price(), 0.12);
        assert_eq!(ev.current_export_price(), 0.0);
        assert_eq!(ev.current_period_name(), "flat");
        let _ = ev.tier_rate_at(0.0);
        assert!(ev.is_finished());
    }

    #[test]
    fn accessor_valid_mid_simulation() {
        let start = make_start(2025, 1, 1);
        let ev = make_evaluator(flat_tariff(0.12), start, start + Duration::hours(3), 3600);
        assert!(!ev.is_finished());
        assert_eq!(ev.current_price(), 0.12);
        assert_eq!(ev.current_export_price(), 0.0);
    }

    #[test]
    fn accessor_after_step_returns_none_no_panic() {
        let start = make_start(2025, 1, 1);
        let end = start + Duration::hours(1);
        let interval = 3600u32;
        let mut ev = make_evaluator(flat_tariff(0.12), start, end, interval);

        // Run the single step to completion.
        let step_end = start + Duration::seconds(interval as i64);
        let _ = ev.step(1.0, 0.0, interval as f64, step_end);
        assert!(ev.is_finished());

        // Accessors must not panic even after step() has finished.
        assert_eq!(ev.current_price(), 0.12);
        assert_eq!(ev.current_export_price(), 0.0);
        let _ = ev.current_period_name();
        let _ = ev.tier_rate_at(100.0);
    }

    #[test]
    fn demand_window_30_minutes() {
        use crate::types::DemandRate;

        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 300u32; // 5-minute intervals
        let mut tariff = flat_tariff(0.12);
        tariff.demand_window_minutes = 30;
        tariff.demand_rates = vec![DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: 10.0,
            ratchet: None,
        }];
        let mut ev = make_evaluator(tariff, start, end, interval);

        // Push 6 steps of 10 kW (30 min at 5-min intervals).
        for i in 0..6 {
            let t = start + Duration::seconds((i as i64 + 1) * interval as i64);
            ev.step(10.0, 0.0, interval as f64, t);
        }
        // Then push low load.
        for i in 6..12 {
            let t = start + Duration::seconds((i as i64 + 1) * interval as i64);
            ev.step(1.0, 0.0, interval as f64, t);
        }

        // The peak should be ~10 kW (the 30-min average of the first 6 steps).
        let peak = ev.billing_state().peak_demand_kw();
        assert!(
            (peak - 10.0).abs() < 0.5,
            "30-min window peak should be ~10 kW, got {}",
            peak
        );
    }

    #[test]
    fn demand_window_default_15_minutes() {
        use crate::types::DemandRate;

        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 300u32;
        let mut tariff = flat_tariff(0.12);
        // demand_window_minutes defaults to 0 from Default, evaluator treats as 15
        tariff.demand_rates = vec![DemandRate {
            period_name: None,
            season: SeasonFilter::All,
            rate_per_kw: 10.0,
            ratchet: None,
        }];
        let mut ev = make_evaluator(tariff, start, end, interval);

        // Push 3 steps of 10 kW (15 min at 5-min intervals).
        for i in 0..3 {
            let t = start + Duration::seconds((i as i64 + 1) * interval as i64);
            ev.step(10.0, 0.0, interval as f64, t);
        }
        let peak = ev.billing_state().peak_demand_kw();
        assert!(
            (peak - 10.0).abs() < 0.5,
            "default 15-min window peak should be ~10 kW, got {}",
            peak
        );
    }

    #[test]
    fn evaluator_rejects_zero_step_simulation() {
        // 30 seconds with 3600-second interval = 0 steps.
        let start = make_start(2025, 1, 1);
        let end = start + Duration::seconds(30);
        let result = TariffEvaluator::new(flat_tariff(0.12), start, end, 3600);
        assert!(
            result.is_err(),
            "should reject simulation shorter than one interval"
        );
    }

    #[test]
    fn finalize_fixed_charge_only_no_metered_load() {
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;
        let mut tariff = flat_tariff(0.12);
        tariff.fixed_charges = FixedCharges {
            monthly_usd: 20.0,
            daily_usd: 0.0,
        };
        let mut ev = make_evaluator(tariff, start, end, interval);

        // Step with zero load -- no energy, no demand.
        let step_end = start + Duration::seconds(interval as i64);
        ev.step(0.0, 0.0, interval as f64, step_end);

        let summary = ev
            .finalize(end)
            .expect("should return Some even with zero metered load");
        assert!(
            (summary.fixed_charge_usd - 20.0).abs() < 1e-10,
            "fixed_charge_usd should be $20 monthly, got {}",
            summary.fixed_charge_usd
        );
    }

    #[test]
    fn finalize_sets_finished() {
        let start = make_start(2025, 1, 1);
        let end = start + Duration::hours(2);
        let interval = 3600u32;
        let mut ev = make_evaluator(flat_tariff(0.12), start, end, interval);
        let step_end = start + Duration::seconds(interval as i64);
        ev.step(1.0, 0.0, interval as f64, step_end);

        assert!(!ev.is_finished());
        let _ = ev.finalize(end);
        assert!(ev.is_finished(), "finalize should set finished = true");
    }

    #[test]
    fn evaluator_tiered_billing_blended_cost_at_period_close() {
        // Tiered tariff: 0-500 kWh at $0.10/kWh, 500+ at $0.20/kWh.
        // Flat TOU rate is $0.15 (used step-by-step but overridden at close).
        // Run a full January month at constant 1 kW = 744 kWh total.
        // Expected tiered cost: 500*0.10 + 244*0.20 = 50.00 + 48.80 = $98.80
        use hares_types::SeasonalSplit;

        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;

        let tariff = ElectricTariff {
            name: Some("tiered-test".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.15,
            }],
            tiered_rates: vec![TieredBlock {
                season: SeasonFilter::Winter,
                thresholds_kwh: vec![500.0],
                rates_per_kwh: vec![0.10, 0.20],
            }],
            seasonal_split: Some(SeasonalSplit::new(6, 9).unwrap()),
            ..Default::default()
        };

        let mut ev = make_evaluator(tariff, start, end, interval);
        let summaries = run_all_steps(&mut ev, |_| 1.0);

        assert_eq!(summaries.len(), 1, "expected 1 billing period");
        let s = &summaries[0];

        // January has 744 hours, constant 1 kW = 744 kWh
        assert!(
            (s.total_import_kwh - 744.0).abs() < 1e-6,
            "expected 744 kWh, got {}",
            s.total_import_kwh
        );

        // Tiered: 500 * 0.10 + 244 * 0.20 = 50 + 48.80 = 98.80
        let expected = 500.0 * 0.10 + 244.0 * 0.20;
        assert!(
            (s.energy_charge_usd - expected).abs() < 1e-6,
            "expected tiered energy charge ${expected:.2}, got ${:.2}",
            s.energy_charge_usd
        );
    }

    #[test]
    fn evaluator_minimum_charge_excludes_export_true() {
        // Net-exporting customer: metered charges below minimum, large export credit.
        // With excludes_export=true: net = max(metered, min) - export_credit
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;

        let mut tariff = flat_tariff(0.12);
        tariff.minimum_charge = Some(50.0);
        tariff.minimum_charge_excludes_export = true;
        tariff.export_rate = ExportRate {
            mode: ExportMode::NetMetering,
            tou_credits: vec![],
        };

        let mut ev = make_evaluator(tariff, start, end, interval);

        // Alternate: import 0.05 kW for some hours, export -5 kW for others
        // to get low metered charges but high export credit.
        let summaries = run_all_steps(&mut ev, |i| if i % 3 == 0 { 0.05 } else { -5.0 });

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
        let metered = s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd;

        // metered < 50 since import is tiny, so floor applies:
        // net = max(metered, 50.0) - export_credit = 50.0 - export_credit
        assert!(
            metered < 50.0,
            "metered charges should be below minimum for this test, got {metered}"
        );
        let expected = 50.0_f64.max(metered) - s.export_credit_usd;
        assert!(
            (s.net_bill_usd - expected).abs() < 1e-6,
            "excludes_export=true: expected ${expected:.2}, got ${:.2}",
            s.net_bill_usd
        );
    }

    #[test]
    fn evaluator_minimum_charge_excludes_export_false() {
        // Same scenario but with excludes_export=false:
        // net = max(metered - export_credit, min)
        let start = make_start(2025, 1, 1);
        let end = make_start(2025, 2, 1);
        let interval = 3600u32;

        let mut tariff = flat_tariff(0.12);
        tariff.minimum_charge = Some(50.0);
        tariff.minimum_charge_excludes_export = false;
        tariff.export_rate = ExportRate {
            mode: ExportMode::NetMetering,
            tou_credits: vec![],
        };

        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |i| if i % 3 == 0 { 0.05 } else { -5.0 });

        let s = summaries
            .into_iter()
            .next()
            .expect("billing period should close");
        let metered = s.energy_charge_usd + s.demand_charge_usd + s.fixed_charge_usd;
        let raw_net = metered - s.export_credit_usd;

        // With large export, raw_net should be negative or well below min.
        assert!(
            raw_net < 50.0,
            "raw net should be below minimum for this test, got {raw_net}"
        );
        assert!(
            (s.net_bill_usd - 50.0).abs() < 1e-6,
            "excludes_export=false: net should be floored to $50.00, got ${:.2}",
            s.net_bill_usd
        );
    }

    // ── RTP tests ──────────────────────────────────────────────────────────

    fn rtp_tariff(prices: Vec<f64>) -> ElectricTariff {
        ElectricTariff {
            name: Some("rtp-test".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.10, // fallback — should be overridden by RTP
            }],
            rtp_schedule: Some(prices),
            ..Default::default()
        }
    }

    #[test]
    fn rtp_price_lookup_by_hour_of_year() {
        // Build RTP schedule where price = hour_of_year / 1000.0
        let prices: Vec<f64> = (0..8760).map(|i| i as f64 / 1000.0).collect();
        let tariff = rtp_tariff(prices.clone());

        // Test hour 0: Jan 1 midnight → index 0
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let ev = make_evaluator(tariff.clone(), start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_price() - 0.0).abs() < 1e-10,
            "hour 0 price should be 0.0"
        );

        // Test hour 12: Jan 1 noon → index 12
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap();
        let ev = make_evaluator(tariff.clone(), start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_price() - 0.012).abs() < 1e-10,
            "hour 12 price should be 0.012"
        );

        // Test hour 8759: Dec 31 23:00 → index 8759
        let start = New_York.with_ymd_and_hms(2025, 12, 31, 23, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_price() - 8.759).abs() < 1e-10,
            "hour 8759 price should be 8.759"
        );
    }

    #[test]
    fn rtp_price_wraparound_at_year_boundary() {
        // RTP schedule with distinct values at index 0 and 1.
        let prices: Vec<f64> = (0..8760).map(|i| i as f64 / 1000.0).collect();
        let tariff = rtp_tariff(prices);

        // Hour 0 of year 2 (Jan 1 2026 00:00) should wrap to index 0.
        let start = New_York.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_price() - 0.0).abs() < 1e-10,
            "wraparound: hour 0 of year 2 should use RTP index 0"
        );
    }

    #[test]
    fn rtp_full_year_total_cost_matches_expected() {
        // Use a constant RTP price to avoid DST-related index shifts in the
        // hour-of-year mapping. The test verifies the full integration:
        // RTP schedule is used for all 8760 steps of a non-leap year.
        let prices = vec![0.15; 8760];
        let tariff = rtp_tariff(prices);

        let start = make_start(2025, 1, 1);
        let end = make_start(2026, 1, 1);
        let interval = 3600u32;
        let mut ev = make_evaluator(tariff, start, end, interval);

        // Import 2 kW every hour → 2 kWh per step.
        let summaries = run_all_steps(&mut ev, |_| 2.0);

        // 8760 hours × 2 kWh × $0.15/kWh = $2,628.00
        let total_cost: f64 = summaries.iter().map(|s| s.energy_charge_usd).sum();
        let expected = 8760.0 * 2.0 * 0.15;
        assert!(
            (total_cost - expected).abs() < 1e-6,
            "full-year RTP cost should match n_steps × kWh × price; expected {expected}, got {total_cost}"
        );
    }

    // ── CPP tests ──────────────────────────────────────────────────────────

    fn cpp_tariff(event_rate: f64, event_limit: u32, event_schedule: Vec<i32>) -> ElectricTariff {
        ElectricTariff {
            name: Some("cpp-test".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.10,
            }],
            cpp_config: Some(CppConfig {
                event_rate_per_kwh: event_rate,
                event_count_limit: event_limit,
                event_schedule,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn cpp_event_detected_and_rate_applied() {
        // Mark only hour 6 (6:00 on Jan 1) as a CPP event.
        let mut schedule = vec![0i32; 8760];
        schedule[6] = 1;
        let tariff = cpp_tariff(1.50, 15, schedule);

        // Hour 6:00 → should get CPP rate $1.50
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 6, 0, 0).unwrap();
        let mut ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        let step_end = start + Duration::hours(1);
        ev.step(1.0, 0.0, 3600.0, step_end);

        // The energy cost should be 1 kWh * $1.50 = $1.50
        let summary = ev.finalize(step_end).expect("should return summary");
        assert!(
            (summary.energy_charge_usd - 1.50).abs() < 1e-10,
            "CPP event at hour 6 should charge $1.50/kWh; got {}",
            summary.energy_charge_usd
        );
        assert_eq!(ev.billing_state().running_cpp_event_hours(), 1);
    }

    #[test]
    fn cpp_no_event_hour_uses_standard_rate() {
        let mut schedule = vec![0i32; 8760];
        schedule[6] = 1; // only hour 6 is an event
        let tariff = cpp_tariff(1.50, 15, schedule);

        // Hour 5:00 → not an event, should use standard rate $0.10
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 5, 0, 0).unwrap();
        let mut ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        let step_end = start + Duration::hours(1);
        ev.step(1.0, 0.0, 3600.0, step_end);

        let summary = ev.finalize(step_end).expect("should return summary");
        assert!(
            (summary.energy_charge_usd - 0.10).abs() < 1e-10,
            "non-event hour should use standard rate $0.10/kWh; got {}",
            summary.energy_charge_usd
        );
        assert_eq!(ev.billing_state().running_cpp_event_hours(), 0);
    }

    #[test]
    fn cpp_event_counter_enforcement() {
        // Event hours at 6, 7, 8, 9, 10. Limit is 3.
        let mut schedule = vec![0i32; 8760];
        for v in schedule.iter_mut().skip(6).take(5) {
            *v = 1;
        }
        let tariff = cpp_tariff(1.50, 3, schedule);

        // Simulate hours 0 through 11
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let end = start + Duration::hours(12);
        let mut ev = make_evaluator(tariff, start, end, 3600);

        for i in 0..12 {
            let step_end = start + Duration::seconds((i as i64 + 1) * 3600);
            ev.step(1.0, 0.0, 3600.0, step_end);
        }

        let summary = ev.finalize(end).expect("should return summary");
        // Hours 0-5: standard rate $0.10 → 6 kWh * $0.10 = $0.60
        // Hours 6-8: CPP rate $1.50 → 3 kWh * $1.50 = $4.50   (3 events, then limit hit)
        // Hours 9-10: standard rate $0.10 → 2 kWh * $0.10 = $0.20 (limit exceeded)
        // Hour 11: standard rate $0.10 → 1 kWh * $0.10 = $0.10
        // Total: $0.60 + $4.50 + $0.20 + $0.10 = $5.40
        assert!(
            (summary.energy_charge_usd - 5.40).abs() < 1e-6,
            "enforced CPP limit: expected $5.40, got {}",
            summary.energy_charge_usd
        );
        assert_eq!(
            ev.billing_state().running_cpp_event_hours(),
            3,
            "should have exactly 3 CPP event hours counted"
        );
    }

    #[test]
    fn cpp_full_year_event_limit_enforced() {
        // 15 CPP event hours scattered across the year, but limit is 10.
        let mut schedule = vec![0i32; 8760];
        for h in [
            100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 2000, 3000, 4000, 5000, 6000,
        ] {
            schedule[h] = 1;
        }
        let tariff = cpp_tariff(1.50, 10, schedule);

        let start = make_start(2025, 1, 1);
        let end = make_start(2026, 1, 1);
        let interval = 3600u32;
        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |_| 1.0);

        // 8760 hours total. 10 CPP hours @ $1.50 (matching the limit first),
        // 8750 hours @ $0.10. Total: 10*1.50 + 8750*0.10 = 15 + 875 = $890
        let total: f64 = summaries.iter().map(|s| s.energy_charge_usd).sum();
        let expected = 10.0 * 1.50 + 8750.0 * 0.10;
        assert!(
            (total - expected).abs() < 1e-6,
            "full-year CPP with limit 10: expected {expected}, got {total}"
        );
        assert_eq!(ev.billing_state().running_cpp_event_hours(), 10);
    }

    // ── EV rate test ───────────────────────────────────────────────────────

    #[test]
    fn ev_sub_tariff_rate_applied() {
        let tariff = ElectricTariff {
            name: Some("ev-test".into()),
            tou_schedule: vec![
                TouPeriod {
                    name: "house".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                    season: SeasonFilter::All,
                },
                TouPeriod {
                    name: "ev-off-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                    season: SeasonFilter::All,
                },
            ],
            energy_rates: vec![
                EnergyRate {
                    period_name: "house".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.30,
                },
                EnergyRate {
                    period_name: "ev-off-peak".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.06,
                },
            ],
            ev_tou_period_name: Some("ev-off-peak".into()),
            ..Default::default()
        };

        // Total import 5 kW: 3 kW house + 2 kW EV
        let start = New_York.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap();
        let mut ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        let step_end = start + Duration::hours(1);
        ev.step(5.0, 2.0, 3600.0, step_end);

        let summary = ev.finalize(step_end).expect("should return summary");
        // 3 kWh house @ $0.30 = $0.90, 2 kWh EV @ $0.06 = $0.12
        // Total = $1.02
        assert!(
            (summary.energy_charge_usd - 1.02).abs() < 1e-10,
            "blended EV+house cost should be $1.02; got {}",
            summary.energy_charge_usd
        );
    }

    #[test]
    fn ev_sub_tariff_zero_ev_power_no_effect() {
        let tariff = ElectricTariff {
            name: Some("ev-zero-test".into()),
            tou_schedule: vec![TouPeriod {
                name: "house".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "house".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.30,
            }],
            ev_tou_period_name: Some("house".into()), // EV uses same rate
            ..Default::default()
        };

        let start = New_York.with_ymd_and_hms(2025, 1, 1, 12, 0, 0).unwrap();
        let mut ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        let step_end = start + Duration::hours(1);
        // 5 kW house load, 0 kW EV
        ev.step(5.0, 0.0, 3600.0, step_end);

        let summary = ev.finalize(step_end).expect("should return summary");
        // 5 kWh @ $0.30 = $1.50
        assert!(
            (summary.energy_charge_usd - 1.50).abs() < 1e-10,
            "zero EV power should produce standard cost; got {}",
            summary.energy_charge_usd
        );
    }

    // ── Hourly export schedule tests ────────────────────────────────────────

    fn hourly_schedule_tariff(prices: Vec<f64>) -> ElectricTariff {
        ElectricTariff {
            name: Some("hourly-export-test".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.10,
            }],
            export_rate: ExportRate {
                mode: ExportMode::HourlySchedule(prices),
                tou_credits: vec![],
            },
            ..Default::default()
        }
    }

    #[test]
    fn hourly_export_hour_0_is_jan1_midnight() {
        // Hour 0 = Jan 1 00:00. Index 0 in schedule = $0.05
        let prices: Vec<f64> = (0..8760).map(|i| i as f64 / 1000.0).collect();
        let tariff = hourly_schedule_tariff(prices);

        let start = New_York.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_export_price() - 0.0).abs() < 1e-10,
            "hour 0 export price should be 0.0, got {}",
            ev.current_export_price()
        );
    }

    #[test]
    fn hourly_export_hour_8759_is_dec31_2300() {
        // Hour 8759 = Dec 31 23:00. Index 8759 in schedule = 8.759
        let prices: Vec<f64> = (0..8760).map(|i| i as f64 / 1000.0).collect();
        let tariff = hourly_schedule_tariff(prices);

        let start = New_York.with_ymd_and_hms(2025, 12, 31, 23, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_export_price() - 8.759).abs() < 1e-10,
            "hour 8759 export price should be 8.759, got {}",
            ev.current_export_price()
        );
    }

    #[test]
    fn hourly_export_tou_credits_not_taken() {
        // HourlySchedule with known prices + tou_credits with a different value.
        // The export price must come from the schedule, not tou_credits.
        let mut prices = vec![0.08; 8760];
        prices[6] = 0.15; // hour 6 (6:00 Jan 1) = $0.15

        let tariff = ElectricTariff {
            name: Some("hourly-export-bypass".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![EnergyRate {
                period_name: "flat".into(),
                season: SeasonFilter::All,
                rate_per_kwh: 0.10,
            }],
            export_rate: ExportRate {
                mode: ExportMode::HourlySchedule(prices),
                // These tou_credits should be ignored because HourlySchedule is active.
                tou_credits: vec![EnergyRate {
                    period_name: "flat".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.99, // deliberately wrong to prove ignored
                }],
            },
            ..Default::default()
        };

        let start = New_York.with_ymd_and_hms(2025, 1, 1, 6, 0, 0).unwrap();
        let ev = make_evaluator(tariff, start, start + Duration::hours(1), 3600);
        assert!(
            (ev.current_export_price() - 0.15).abs() < 1e-10,
            "export price should come from hourly schedule (0.15), not tou_credits (0.99); got {}",
            ev.current_export_price()
        );
    }

    #[test]
    fn hourly_export_full_year_total_matches_expected() {
        // Full year, constant 1 kW export at each hour.
        // Use a constant export price of $0.08/kWh for all hours.
        // Total export credit = 8760 kWh * $0.08/kWh = $700.80
        let prices = vec![0.08; 8760];
        let tariff = hourly_schedule_tariff(prices);

        let start = make_start(2025, 1, 1);
        let end = make_start(2026, 1, 1);
        let interval = 3600u32;
        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |_| -1.0);

        let total_export_credit: f64 = summaries.iter().map(|s| s.export_credit_usd).sum();
        let expected = 8760.0 * 0.08;
        assert!(
            (total_export_credit - expected).abs() < 1e-6,
            "full-year export credit should be {expected}; got {total_export_credit}"
        );
    }

    #[test]
    fn hourly_export_full_year_variable_prices() {
        // Export prices vary by hour: price[i] = 0.05 if i < 4380, else 0.15.
        // Export 1 kW every hour.
        // Expected total (ignoring DST shifts): 4380*0.05 + 4380*0.15 = 876.00.
        // DST spring-forward skips one hour-of-year, fall-back duplicates one;
        // this can shift up to one hour between price bands, so the total may
        // differ by ±$0.10. Use a tolerance of $0.50 to accommodate.
        let mut prices = vec![0.05; 8760];
        for v in prices.iter_mut().skip(4380) {
            *v = 0.15;
        }
        let tariff = hourly_schedule_tariff(prices);

        let start = make_start(2025, 1, 1);
        let end = make_start(2026, 1, 1);
        let interval = 3600u32;
        let mut ev = make_evaluator(tariff, start, end, interval);

        let summaries = run_all_steps(&mut ev, |_| -1.0);

        let total_export_credit: f64 = summaries.iter().map(|s| s.export_credit_usd).sum();
        let expected = 4380.0 * 0.05 + 4380.0 * 0.15;
        assert!(
            (total_export_credit - expected).abs() < 0.50,
            "variable-price export credit should be ~{expected}; got {total_export_credit}"
        );
    }

    #[test]
    fn hourly_export_backward_compat_net_billing_tou_credits() {
        // NetBilling mode with tou_credits but NO HourlySchedule.
        // Must produce the same result as before the change.
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
        // Export price should come from tou_credits: 0.08
        assert_eq!(ev.current_export_price(), 0.08);
    }

    #[test]
    fn evaluator_southern_hemisphere_seasonal_split_applies_summer_rates_in_january() {
        use hares_types::SeasonalSplit;

        let southern_summer = SeasonalSplit::new(12, 2).unwrap();
        let tariff = ElectricTariff {
            name: Some("southern-hemi".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![
                EnergyRate {
                    period_name: "flat".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: SUMMER_PEAK,
                },
                EnergyRate {
                    period_name: "flat".into(),
                    season: SeasonFilter::Winter,
                    rate_per_kwh: WINTER_PEAK,
                },
            ],
            seasonal_split: Some(southern_summer),
            ..Default::default()
        };

        // January 6, 2025 (Monday) — should be summer in southern hemisphere.
        let jan = New_York.with_ymd_and_hms(2025, 1, 6, 12, 0, 0).unwrap();
        let ev_jan =
            TariffEvaluator::new(tariff.clone(), jan, jan + Duration::hours(1), 3600).unwrap();
        assert_eq!(
            ev_jan.current_price(),
            SUMMER_PEAK,
            "January should use summer rate with southern hemisphere split"
        );

        // July 7, 2025 (Monday) — should be winter in southern hemisphere.
        let jul = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev_jul =
            TariffEvaluator::new(tariff.clone(), jul, jul + Duration::hours(1), 3600).unwrap();
        assert_eq!(
            ev_jul.current_price(),
            WINTER_PEAK,
            "July should use winter rate with southern hemisphere split"
        );
    }

    #[test]
    fn evaluator_seasonal_split_none_fallback_june_september() {
        // Split=None → June–September default.
        let tariff = ElectricTariff {
            name: Some("no-split".into()),
            tou_schedule: vec![TouPeriod {
                name: "flat".into(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![
                EnergyRate {
                    period_name: "flat".into(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: SUMMER_PEAK,
                },
                EnergyRate {
                    period_name: "flat".into(),
                    season: SeasonFilter::Winter,
                    rate_per_kwh: WINTER_PEAK,
                },
            ],
            seasonal_split: None,
            ..Default::default()
        };

        // July 7 (Monday) — summer per June–September default.
        let jul = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev_jul =
            TariffEvaluator::new(tariff.clone(), jul, jul + Duration::hours(1), 3600).unwrap();
        assert_eq!(ev_jul.current_price(), SUMMER_PEAK);

        // January 6 (Monday) — winter per June–September default.
        let jan = New_York.with_ymd_and_hms(2025, 1, 6, 12, 0, 0).unwrap();
        let ev_jan =
            TariffEvaluator::new(tariff.clone(), jan, jan + Duration::hours(1), 3600).unwrap();
        assert_eq!(ev_jan.current_price(), WINTER_PEAK);
    }

    #[test]
    fn evaluator_seasonal_split_demand_charge_respects_split() {
        use crate::types::DemandRate;
        use hares_types::SeasonalSplit;

        let southern_summer = SeasonalSplit::new(12, 2).unwrap();

        fn make_demand_tariff(split: Option<SeasonalSplit>) -> ElectricTariff {
            let summer_rate = 10.0;
            let winter_rate = 5.0;
            ElectricTariff {
                name: Some("seasonal-demand".into()),
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
                demand_rates: vec![
                    DemandRate {
                        period_name: None,
                        season: SeasonFilter::Summer,
                        rate_per_kw: summer_rate,
                        ratchet: None,
                    },
                    DemandRate {
                        period_name: None,
                        season: SeasonFilter::Winter,
                        rate_per_kw: winter_rate,
                        ratchet: None,
                    },
                ],
                seasonal_split: split,
                ..Default::default()
            }
        }

        // January — summer per southern hemisphere split (months 12, 1, 2).
        let jan_start = make_start(2025, 1, 1);
        let jan_end = make_start(2025, 2, 1);
        let mut ev_jan = make_evaluator(
            make_demand_tariff(Some(southern_summer)),
            jan_start,
            jan_end,
            3600,
        );
        let summaries_jan = run_all_steps(&mut ev_jan, |_| 3.0);
        let s = summaries_jan
            .into_iter()
            .next()
            .expect("billing period should close");
        // Peak = 3 kW, season = Summer => demand charge = 3 * 10 = 30
        assert!(
            (s.demand_charge_usd - 30.0).abs() < 0.01,
            "January demand charge should use summer rate (30.0), got {}",
            s.demand_charge_usd
        );

        // July — winter per southern hemisphere split.
        let jul_start = make_start(2025, 7, 1);
        let jul_end = make_start(2025, 8, 1);
        let mut ev_jul = make_evaluator(
            make_demand_tariff(Some(southern_summer)),
            jul_start,
            jul_end,
            3600,
        );
        let summaries_jul = run_all_steps(&mut ev_jul, |_| 3.0);
        let s = summaries_jul
            .into_iter()
            .next()
            .expect("billing period should close");
        // Peak = 3 kW, season = Winter => demand charge = 3 * 5 = 15
        assert!(
            (s.demand_charge_usd - 15.0).abs() < 0.01,
            "July demand charge should use winter rate (15.0), got {}",
            s.demand_charge_usd
        );
    }

    #[test]
    fn evaluator_seasonal_split_tiered_rate_respects_split() {
        use hares_types::SeasonalSplit;

        let southern_summer = SeasonalSplit::new(12, 2).unwrap();
        let tariff = ElectricTariff {
            name: Some("southern-hemi-tiered".into()),
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
            seasonal_split: Some(southern_summer),
            ..Default::default()
        };

        // January (summer in southern hemisphere) → summer tiered block.
        let jan = New_York.with_ymd_and_hms(2025, 1, 6, 12, 0, 0).unwrap();
        let ev_jan =
            TariffEvaluator::new(tariff.clone(), jan, jan + Duration::hours(1), 3600).unwrap();
        assert_eq!(ev_jan.tier_rate_at(0.0), 0.10);
        assert_eq!(ev_jan.tier_rate_at(600.0), 0.20);

        // July (winter in southern hemisphere) → winter tiered block.
        let jul = New_York.with_ymd_and_hms(2025, 7, 7, 12, 0, 0).unwrap();
        let ev_jul =
            TariffEvaluator::new(tariff.clone(), jul, jul + Duration::hours(1), 3600).unwrap();
        assert_eq!(ev_jul.tier_rate_at(0.0), 0.08);
        assert_eq!(ev_jul.tier_rate_at(800.0), 0.15);
    }

    #[test]
    fn three_season_tariff_produces_correct_seasonal_costs() {
        // Tariff with three distinct seasons:
        //   Summer (Jun-Sep): $0.30/kWh
        //   Shoulder (Apr-May, Oct): $0.20/kWh
        //   Winter (Nov-Mar): $0.10/kWh
        let seasonal_split = SeasonalSplit::with_shoulder(6, 9, Some(4), Some(5)).unwrap();

        let period_name = "all_hours".to_string();
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: period_name.clone(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![
                EnergyRate {
                    period_name: period_name.clone(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: 0.30,
                },
                EnergyRate {
                    period_name: period_name.clone(),
                    season: SeasonFilter::Shoulder,
                    rate_per_kwh: 0.20,
                },
                EnergyRate {
                    period_name,
                    season: SeasonFilter::Winter,
                    rate_per_kwh: 0.10,
                },
            ],
            seasonal_split: Some(seasonal_split),
            billing_cycle: BillingCycle::Monthly,
            ..Default::default()
        };

        // August (summer) — run 1 hour
        let aug = New_York.with_ymd_and_hms(2025, 8, 15, 12, 0, 0).unwrap();
        let ev_aug =
            TariffEvaluator::new(tariff.clone(), aug, aug + Duration::hours(1), 3600).unwrap();
        assert!((ev_aug.current_price() - 0.30).abs() < 1e-9);

        // December (winter) — run 1 hour
        let dec = New_York.with_ymd_and_hms(2025, 12, 15, 12, 0, 0).unwrap();
        let ev_dec =
            TariffEvaluator::new(tariff.clone(), dec, dec + Duration::hours(1), 3600).unwrap();
        assert!((ev_dec.current_price() - 0.10).abs() < 1e-9);

        // April (shoulder) — run 1 hour
        let apr = New_York.with_ymd_and_hms(2025, 4, 15, 12, 0, 0).unwrap();
        let ev_apr =
            TariffEvaluator::new(tariff.clone(), apr, apr + Duration::hours(1), 3600).unwrap();
        assert!((ev_apr.current_price() - 0.20).abs() < 1e-9);
    }

    #[test]
    fn three_season_tariff_billing_over_three_seasons() {
        // Full year: verify costs across summer, shoulder, and winter months.
        // 1 kWh in each hour for 1 hour per representative month.
        // summer=Jun-Sep(6-9), shoulder=Apr-May(4-5), winter=Nov-Mar(11-3) + Oct(10)
        let seasonal_split = SeasonalSplit::with_shoulder(6, 9, Some(4), Some(5)).unwrap();

        let period_name = "peak".to_string();
        let tariff = ElectricTariff {
            tou_schedule: vec![TouPeriod {
                name: period_name.clone(),
                schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                season: SeasonFilter::All,
            }],
            energy_rates: vec![
                EnergyRate {
                    period_name: period_name.clone(),
                    season: SeasonFilter::Summer,
                    rate_per_kwh: 0.30,
                },
                EnergyRate {
                    period_name: period_name.clone(),
                    season: SeasonFilter::Shoulder,
                    rate_per_kwh: 0.20,
                },
                EnergyRate {
                    period_name,
                    season: SeasonFilter::Winter,
                    rate_per_kwh: 0.10,
                },
            ],
            seasonal_split: Some(seasonal_split),
            billing_cycle: BillingCycle::Monthly,
            ..Default::default()
        };

        // July (summer): 1 kWh → $0.30
        let jul = New_York.with_ymd_and_hms(2025, 7, 15, 12, 0, 0).unwrap();
        let mut ev =
            TariffEvaluator::new(tariff.clone(), jul, jul + Duration::hours(1), 3600).unwrap();
        ev.step(1.0, 0.0, 3600.0, jul);
        let summary = ev.finalize(jul + Duration::hours(1)).unwrap();
        assert!((summary.energy_charge_usd - 0.30).abs() < 0.01);

        // April (shoulder): 1 kWh → $0.20
        let apr = New_York.with_ymd_and_hms(2025, 4, 15, 12, 0, 0).unwrap();
        let mut ev =
            TariffEvaluator::new(tariff.clone(), apr, apr + Duration::hours(1), 3600).unwrap();
        ev.step(1.0, 0.0, 3600.0, apr);
        let summary = ev.finalize(apr + Duration::hours(1)).unwrap();
        assert!((summary.energy_charge_usd - 0.20).abs() < 0.01);

        // December (winter): 1 kWh → $0.10
        let dec = New_York.with_ymd_and_hms(2025, 12, 15, 12, 0, 0).unwrap();
        let mut ev =
            TariffEvaluator::new(tariff.clone(), dec, dec + Duration::hours(1), 3600).unwrap();
        ev.step(1.0, 0.0, 3600.0, dec);
        let summary = ev.finalize(dec + Duration::hours(1)).unwrap();
        assert!((summary.energy_charge_usd - 0.10).abs() < 0.01);
    }
}
