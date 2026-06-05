use std::collections::{BTreeMap, BTreeSet};

use hares_types::{BillingCycle, DayFilter, SeasonFilter, SeasonalSplit, TimeWindow, TouPeriod};
use serde_json::Value;

use crate::{
    DemandRate, ElectricTariff, EnergyRate, ExportMode, ExportRate, FixedCharges, RatchetConfig,
    TieredBlock,
};

#[derive(Debug, Clone)]
pub struct UrdbParseError {
    pub message: String,
}

impl std::fmt::Display for UrdbParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "URDB parse error: {}", self.message)
    }
}

impl std::error::Error for UrdbParseError {}

impl UrdbParseError {
    fn missing(field: &str) -> Self {
        Self {
            message: format!("missing required field '{field}'"),
        }
    }

    fn malformed(field: &str, detail: &str) -> Self {
        Self {
            message: format!("malformed field '{field}': {detail}"),
        }
    }
}

type Schedule = Vec<Vec<u64>>;

fn extract_schedule(root: &Value, field: &str) -> Result<Schedule, UrdbParseError> {
    let arr = root
        .get(field)
        .ok_or_else(|| UrdbParseError::missing(field))?;
    let months = arr
        .as_array()
        .ok_or_else(|| UrdbParseError::malformed(field, "expected array of 12 arrays"))?;
    if months.len() != 12 {
        return Err(UrdbParseError::malformed(
            field,
            &format!("expected 12 month rows, got {}", months.len()),
        ));
    }
    let mut result = Vec::with_capacity(12);
    for (m, month_val) in months.iter().enumerate() {
        let hours = month_val.as_array().ok_or_else(|| {
            UrdbParseError::malformed(field, &format!("month {m} is not an array"))
        })?;
        if hours.len() != 24 {
            return Err(UrdbParseError::malformed(
                field,
                &format!("month {m} has {} hours, expected 24", hours.len()),
            ));
        }
        let mut row = Vec::with_capacity(24);
        for (h, hval) in hours.iter().enumerate() {
            let idx = hval.as_u64().ok_or_else(|| {
                UrdbParseError::malformed(field, &format!("month {m} hour {h} is not an integer"))
            })?;
            row.push(idx);
        }
        result.push(row);
    }
    Ok(result)
}

fn try_extract_schedule(root: &Value, field: &str) -> Result<Option<Schedule>, UrdbParseError> {
    if root.get(field).is_none() {
        return Ok(None);
    }
    extract_schedule(root, field).map(Some)
}

fn unique_period_indices(weekday: &Schedule, weekend: &Schedule) -> BTreeSet<u64> {
    let mut indices = BTreeSet::new();
    for row in weekday.iter().chain(weekend.iter()) {
        for &idx in row {
            indices.insert(idx);
        }
    }
    indices
}

/// For a given period index, determine which months (1-indexed) use it.
fn months_for_period(weekday: &Schedule, weekend: &Schedule, period_idx: u64) -> BTreeSet<u8> {
    let mut months = BTreeSet::new();
    for (m, row) in weekday.iter().enumerate() {
        if row.contains(&period_idx) {
            months.insert((m + 1) as u8);
        }
    }
    for (m, row) in weekend.iter().enumerate() {
        if row.contains(&period_idx) {
            months.insert((m + 1) as u8);
        }
    }
    months
}

/// Derive summer months from schedule data by comparing each month's rate
/// periods against December (the reference winter month). Months whose
/// 24-hour period pattern differs from December are classified as summer.
/// Returns `None` if all months have identical patterns (no seasonal split).
///
/// Works for both hemispheres: if more than 6 months differ from December,
/// December is likely a summer month (southern hemisphere), so the set is
/// inverted -- the months matching December become summer.
fn detect_summer_months(weekday: &Schedule, weekend: &Schedule) -> Option<BTreeSet<u8>> {
    // December is index 11 (month 12, 0-indexed)
    let dec_wd = weekday.get(11)?;
    let dec_we = weekend.get(11)?;

    let mut differ_from_dec = BTreeSet::new();
    for m in 0..12u8 {
        let wd = &weekday[m as usize];
        let we = &weekend[m as usize];
        if wd != dec_wd || we != dec_we {
            differ_from_dec.insert(m + 1); // 1-indexed
        }
    }

    if differ_from_dec.is_empty() {
        return None;
    }

    // If more than 6 months differ from December, December is likely summer
    // (southern hemisphere). Invert: months matching December are winter,
    // so the complement (months that differ) would be winter -- take the rest.
    let summer = if differ_from_dec.len() > 6 {
        let all: BTreeSet<u8> = (1..=12).collect();
        all.difference(&differ_from_dec).copied().collect()
    } else {
        differ_from_dec
    };

    if summer.is_empty() {
        None
    } else {
        Some(summer)
    }
}

/// Build a `SeasonalSplit` from a set of summer months.
///
/// Finds a contiguous range (possibly wrapping around Dec-Jan) that covers all
/// months in the set. If the set is non-contiguous (gaps that can't be explained
/// by wrapping), logs a warning and falls back to the default June-September split.
fn seasonal_split_from_months(summer_months: &BTreeSet<u8>) -> Option<SeasonalSplit> {
    if summer_months.is_empty() {
        return None;
    }

    let months: Vec<u8> = summer_months.iter().copied().collect();
    let n = months.len();

    // Single month: start == end
    if n == 1 {
        return SeasonalSplit::new(months[0], months[0]).ok();
    }

    // Try non-wrapping: check if months form a contiguous sequence
    let min = months[0];
    let max = months[n - 1];
    let non_wrapping_contiguous = (max - min + 1) as usize == n;
    if non_wrapping_contiguous {
        return SeasonalSplit::new(min, max).ok();
    }

    // Try wrapping (e.g., {11, 12, 1, 2}): find a rotation where months are contiguous.
    // Check if the gap is a single contiguous block of non-summer months.
    let all: BTreeSet<u8> = (1..=12).collect();
    let winter: Vec<u8> = all.difference(summer_months).copied().collect();
    if !winter.is_empty() {
        let w_min = winter[0];
        let w_max = winter[winter.len() - 1];
        let winter_contiguous = (w_max - w_min + 1) as usize == winter.len();
        if winter_contiguous {
            // Summer wraps: starts after winter ends, ends before winter starts
            let start = if w_max < 12 { w_max + 1 } else { 1 };
            let end = if w_min > 1 { w_min - 1 } else { 12 };
            return SeasonalSplit::new(start, end).ok();
        }
    }

    // Non-contiguous even with wrapping -- fall back to default
    tracing::warn!(
        ?summer_months,
        "URDB detected non-contiguous summer months; falling back to June-September"
    );
    SeasonalSplit::new(6, 9).ok()
}

fn season_for_months(months: &BTreeSet<u8>, summer_months: &BTreeSet<u8>) -> SeasonFilter {
    let has_summer = months.iter().any(|m| summer_months.contains(m));
    let has_winter = months.iter().any(|m| !summer_months.contains(m));
    match (has_summer, has_winter) {
        (true, false) => SeasonFilter::Summer,
        (false, true) => SeasonFilter::Winter,
        _ => SeasonFilter::All,
    }
}

/// Detect an explicit season label from a `flatdemandstructure` tier object.
/// Checks common field names (`season`, `seasonId`, `season_name`, `seasonid`).
/// Returns `None` if no recognized label is present.
fn season_from_tier_label(tier: &Value) -> Option<SeasonFilter> {
    for field in &["season", "seasonId", "season_name", "seasonid"] {
        if let Some(s) = tier.get(field).and_then(Value::as_str) {
            return Some(match s.to_lowercase().as_str() {
                "summer" => SeasonFilter::Summer,
                "winter" => SeasonFilter::Winter,
                "all" | "both" => SeasonFilter::All,
                other => {
                    tracing::warn!(
                        field,
                        value = other,
                        "unknown season label in flatdemandstructure tier; treating as All"
                    );
                    SeasonFilter::All
                }
            });
        }
    }
    None
}

/// Find contiguous hour ranges for a period in a single schedule matrix,
/// across the months that use that period. Uses the union of all active
/// hours; warns if months have different hour patterns for the same period.
fn hour_ranges_for_period(schedule: &Schedule, period_idx: u64) -> Vec<(u16, u16)> {
    let mut active_hours: BTreeSet<u8> = BTreeSet::new();
    let mut per_month_hours: Vec<BTreeSet<u8>> = Vec::new();
    for row in schedule {
        let mut month_hours = BTreeSet::new();
        for (h, &val) in row.iter().enumerate() {
            if val == period_idx {
                active_hours.insert(h as u8);
                month_hours.insert(h as u8);
            }
        }
        if !month_hours.is_empty() {
            per_month_hours.push(month_hours);
        }
    }
    if per_month_hours.len() > 1 {
        let first = &per_month_hours[0];
        if per_month_hours.iter().any(|m| m != first) {
            tracing::warn!(
                period_idx,
                "URDB period has different hour patterns across months; using union of all hours"
            );
        }
    }

    // Group contiguous hours into ranges.
    let mut ranges = Vec::new();
    let hours: Vec<u8> = active_hours.into_iter().collect();
    if hours.is_empty() {
        return ranges;
    }

    let mut start = hours[0] as u16;
    let mut end = hours[0] as u16;
    for &h in &hours[1..] {
        let h16 = h as u16;
        if h16 == end + 1 {
            end = h16;
        } else {
            ranges.push((start * 60, (end + 1) * 60));
            start = h16;
            end = h16;
        }
    }
    ranges.push((start * 60, (end + 1) * 60));
    ranges
}

fn build_time_windows(
    weekday_ranges: &[(u16, u16)],
    weekend_ranges: &[(u16, u16)],
) -> Vec<TimeWindow> {
    let mut windows = Vec::new();

    // Find ranges that are identical in both weekday and weekend -> DayFilter::Any
    let mut weekday_used = vec![false; weekday_ranges.len()];
    let mut weekend_used = vec![false; weekend_ranges.len()];

    for (wi, &wr) in weekday_ranges.iter().enumerate() {
        for (ei, &er) in weekend_ranges.iter().enumerate() {
            if wr == er && !weekend_used[ei] {
                windows.push(TimeWindow::new(DayFilter::Any, wr.0, wr.1, 0.0));
                weekday_used[wi] = true;
                weekend_used[ei] = true;
                break;
            }
        }
    }

    for (i, &r) in weekday_ranges.iter().enumerate() {
        if !weekday_used[i] {
            windows.push(TimeWindow::new(DayFilter::Weekdays, r.0, r.1, 0.0));
        }
    }

    for (i, &r) in weekend_ranges.iter().enumerate() {
        if !weekend_used[i] {
            windows.push(TimeWindow::new(DayFilter::Weekends, r.0, r.1, 0.0));
        }
    }

    windows
}

fn extract_rate_tiers(
    root: &Value,
    field: &str,
) -> Result<Option<Vec<Vec<Value>>>, UrdbParseError> {
    let Some(val) = root.get(field) else {
        return Ok(None);
    };
    let periods = val
        .as_array()
        .ok_or_else(|| UrdbParseError::malformed(field, "expected array of arrays"))?;
    let mut result = Vec::with_capacity(periods.len());
    for (i, period_val) in periods.iter().enumerate() {
        let tiers = period_val.as_array().ok_or_else(|| {
            UrdbParseError::malformed(field, &format!("period {i} is not an array"))
        })?;
        result.push(tiers.clone());
    }
    Ok(Some(result))
}

fn tier_rate(tier: &Value) -> f64 {
    let rate = tier.get("rate").and_then(Value::as_f64).unwrap_or(0.0);
    let adj = tier.get("adj").and_then(Value::as_f64).unwrap_or(0.0);
    let mut total = rate + adj;

    // URDB v7 may encode demand charges as separate generation/transmission/distribution
    // components (e.g. PG&E splits demand into generation and distribution charges).
    // Each field that is present and parses as an f64 is added to the total.
    for field in &["genrate", "transrate", "distrate"] {
        if let Some(val) = tier.get(field).and_then(Value::as_f64) {
            tracing::debug!(field, val, "URDB additional demand component found");
            total += val;
        }
    }

    total
}

fn tier_max(tier: &Value) -> Option<f64> {
    tier.get("max").and_then(Value::as_f64).filter(|&v| v > 0.0)
}

fn tier_sell(tier: &Value) -> Option<f64> {
    tier.get("sell").and_then(Value::as_f64)
}

fn parse_export_mode(root: &Value) -> ExportMode {
    match root.get("dgrules").and_then(Value::as_str) {
        Some("Net Metering") => ExportMode::NetMetering,
        Some("Net Billing Instantaneous" | "Net Billing Hourly") => ExportMode::NetBilling,
        Some("Buy All Sell All") => {
            // Try to find a sell rate from energy rate structure
            let sell_rate = root
                .get("energyratestructure")
                .and_then(Value::as_array)
                .and_then(|periods| periods.first())
                .and_then(Value::as_array)
                .and_then(|tiers| tiers.first())
                .and_then(tier_sell)
                .unwrap_or(0.0);
            ExportMode::FlatRate(sell_rate)
        }
        _ => ExportMode::None,
    }
}

fn parse_fixed_charges(root: &Value) -> FixedCharges {
    let units = root
        .get("fixedchargeunits")
        .and_then(Value::as_str)
        .unwrap_or("$/month");

    let first = root
        .get("fixedchargefirstmeter")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let second = root
        .get("fixedchargesecondmeter")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    let has_first = root.get("fixedchargefirstmeter").is_some();
    let has_second = root.get("fixedchargesecondmeter").is_some();

    let second_units = root
        .get("fixedchargesecondmeterunits")
        .and_then(Value::as_str);

    let is_daily = |u: &str| u == "$/day";

    let (daily, monthly) = if has_first && has_second {
        if let Some(su) = second_units {
            let (d1, m1) = if is_daily(units) {
                (first, 0.0)
            } else {
                (0.0, first)
            };
            let (d2, m2) = if is_daily(su) {
                (second, 0.0)
            } else {
                (0.0, second)
            };
            tracing::debug!(
                first_meter = first,
                first_units = units,
                second_meter = second,
                second_units = su,
                total_daily = d1 + d2,
                total_monthly = m1 + m2,
                "both fixed charge meters parsed with independent units"
            );
            (d1 + d2, m1 + m2)
        } else {
            let total = first + second;
            tracing::debug!(
                first_meter = first,
                second_meter = second,
                total,
                units,
                "both fixed charge meters parsed and summed"
            );
            if is_daily(units) {
                (total, 0.0)
            } else {
                (0.0, total)
            }
        }
    } else {
        let total = first + second;
        if is_daily(units) {
            (total, 0.0)
        } else {
            (0.0, total)
        }
    };

    if root.get("fixedchargefirstmetergroup").is_some()
        || root.get("fixedchargesecondmetergroup").is_some()
    {
        tracing::warn!(
            "seasonal fixed charges detected (fixedchargefirstmetergroup or fixedchargesecondmetergroup) but not yet supported; only flat fixed charges are parsed"
        );
    }

    FixedCharges {
        daily_usd: daily,
        monthly_usd: monthly,
    }
}

fn warn_unsupported(root: &Value) {
    for field in ["reactivepowercharge", "voltagecategory", "phasewiring"] {
        if root.get(field).is_some() {
            tracing::warn!(field, "unsupported URDB field ignored");
        }
    }
}

/// Parse a URDB v7 JSON string into an `ElectricTariff`.
///
/// Returns `Err` only for structurally invalid JSON (missing required fields,
/// malformed matrices). Unsupported fields produce `tracing::warn!` but not errors.
pub fn parse(json: &str) -> Result<ElectricTariff, UrdbParseError> {
    let root: Value = serde_json::from_str(json)
        .map_err(|e| UrdbParseError::malformed("(root)", &e.to_string()))?;

    warn_unsupported(&root);

    let weekday_sched = extract_schedule(&root, "energyweekdayschedule")?;
    let weekend_sched = extract_schedule(&root, "energyweekendschedule")?;

    let energy_structure = extract_rate_tiers(&root, "energyratestructure")?
        .ok_or_else(|| UrdbParseError::missing("energyratestructure"))?;

    let period_indices = unique_period_indices(&weekday_sched, &weekend_sched);

    // Detect summer months from schedule data, defaulting to June-September.
    let default_summer: BTreeSet<u8> = (6..=9).collect();
    let summer_months = detect_summer_months(&weekday_sched, &weekend_sched)
        .unwrap_or_else(|| default_summer.clone());

    let tariff_name = root.get("name").and_then(Value::as_str).map(String::from);

    // Build TOU periods and energy rates
    let mut tou_schedule = Vec::new();
    let mut energy_rates = Vec::new();
    let mut tiered_rates = Vec::new();
    // Track sell rates for export TOU credits
    let mut sell_credits: Vec<EnergyRate> = Vec::new();

    // Map period indices to their season for demand rate usage
    let mut period_seasons: BTreeMap<u64, SeasonFilter> = BTreeMap::new();

    for &period_idx in &period_indices {
        let period_name = format!("period_{period_idx}");
        let months = months_for_period(&weekday_sched, &weekend_sched, period_idx);
        let season = season_for_months(&months, &summer_months);
        period_seasons.insert(period_idx, season);

        let weekday_ranges = hour_ranges_for_period(&weekday_sched, period_idx);
        let weekend_ranges = hour_ranges_for_period(&weekend_sched, period_idx);
        let windows = build_time_windows(&weekday_ranges, &weekend_ranges);

        if !windows.is_empty() {
            tou_schedule.push(TouPeriod {
                name: period_name.clone(),
                schedule: windows,
                season,
            });
        }

        // Energy rates from energyratestructure[period_idx]
        let idx = period_idx as usize;
        if idx < energy_structure.len() {
            let tiers = &energy_structure[idx];
            let base_rate = tiers.first().map(tier_rate).unwrap_or(0.0);

            energy_rates.push(EnergyRate {
                period_name: period_name.clone(),
                season,
                rate_per_kwh: base_rate,
            });

            // Check for sell rate on first tier
            if let Some(sell) = tiers.first().and_then(tier_sell) {
                sell_credits.push(EnergyRate {
                    period_name: period_name.clone(),
                    season,
                    rate_per_kwh: sell,
                });
            }

            // Build tiered block if multiple tiers
            if tiers.len() > 1 {
                let mut thresholds = Vec::new();
                let mut rates = Vec::new();
                for tier in tiers {
                    rates.push(tier_rate(tier));
                    if let Some(max_kwh) = tier_max(tier) {
                        thresholds.push(max_kwh);
                    }
                }
                if rates.len() == thresholds.len() + 1 {
                    tiered_rates.push(TieredBlock {
                        season,
                        thresholds_kwh: thresholds,
                        rates_per_kwh: rates,
                    });
                } else {
                    tracing::warn!(
                        period = %period_name,
                        rates = rates.len(),
                        thresholds = thresholds.len(),
                        "tiered block invariant not satisfied (rates != thresholds+1); block dropped"
                    );
                }
            }
        }
    }

    // Demand rates
    let mut demand_rates = Vec::new();
    let ratchet = root
        .get("demandratchetpercentage")
        .and_then(Value::as_f64)
        .filter(|&pct| pct > 0.0)
        .map(|pct| RatchetConfig {
            // 11-month lookback (12 months excluding current) is the US utility standard.
            lookback_months: 11,
            minimum_fraction: pct / 100.0,
        });

    // Flat demand (non-TOU). Prefer explicit season labels in tier objects;
    // fall back to array-position heuristic only when no labels are present.
    if let Some(flat_demand) = extract_rate_tiers(&root, "flatdemandstructure")? {
        let num_entries = flat_demand.len();

        // Collect explicit season labels from the first tier of each entry.
        let label_seasons: Vec<Option<SeasonFilter>> = flat_demand
            .iter()
            .map(|tiers| tiers.first().and_then(season_from_tier_label))
            .collect();
        let has_explicit_labels = label_seasons.iter().any(Option::is_some);

        for (idx, tiers) in flat_demand.iter().enumerate() {
            let rate = tiers.first().map(tier_rate).unwrap_or(0.0);
            if rate > 0.0 {
                let season = if has_explicit_labels {
                    label_seasons[idx].unwrap_or_else(|| {
                        tracing::warn!(
                            index = idx,
                            "flatdemandstructure entry has no season label while other entries do; using array-position fallback"
                        );
                        match idx {
                            0 => SeasonFilter::Summer,
                            _ => SeasonFilter::Winter,
                        }
                    })
                } else if num_entries == 1 {
                    SeasonFilter::All
                } else {
                    match idx {
                        0 => {
                            tracing::warn!(
                                tariff = ?tariff_name,
                                "flatdemandstructure has {} entries; season inferred from array position (entry 0 assumed Summer) because no explicit season labels found",
                                num_entries
                            );
                            SeasonFilter::Summer
                        }
                        1 => SeasonFilter::Winter,
                        _ => {
                            tracing::warn!(
                                index = idx,
                                "flatdemandstructure has >2 entries; assigning All to extra entry"
                            );
                            SeasonFilter::All
                        }
                    }
                };
                demand_rates.push(DemandRate {
                    period_name: None,
                    season,
                    rate_per_kw: rate,
                    ratchet: ratchet.clone(),
                });
            }
        }
    }

    // TOU demand
    let mut demand_tou_schedule: Vec<TouPeriod> = Vec::new();
    if let Some(tou_demand) = extract_rate_tiers(&root, "demandratestructure")? {
        let demand_weekday = try_extract_schedule(&root, "demandweekdayschedule")?;
        let demand_weekend = try_extract_schedule(&root, "demandweekendschedule")?;

        // Build demand TOU periods from the demand schedule matrices.
        if let (Some(dwd), Some(dwe)) = (&demand_weekday, &demand_weekend) {
            let demand_period_indices = unique_period_indices(dwd, dwe);
            for &period_idx in &demand_period_indices {
                let period_name = format!("demand_{period_idx}");
                let months = months_for_period(dwd, dwe, period_idx);
                let season = season_for_months(&months, &summer_months);
                let weekday_ranges = hour_ranges_for_period(dwd, period_idx);
                let weekend_ranges = hour_ranges_for_period(dwe, period_idx);
                let windows = build_time_windows(&weekday_ranges, &weekend_ranges);
                if !windows.is_empty() {
                    demand_tou_schedule.push(TouPeriod {
                        name: period_name,
                        schedule: windows,
                        season,
                    });
                }
            }
        }

        for (idx, tiers) in tou_demand.iter().enumerate() {
            let rate = tiers.first().map(tier_rate).unwrap_or(0.0);
            if rate > 0.0 {
                let period_name = format!("demand_{idx}");
                let season = if let (Some(wd), Some(we)) = (&demand_weekday, &demand_weekend) {
                    let months = months_for_period(wd, we, idx as u64);
                    season_for_months(&months, &summer_months)
                } else {
                    SeasonFilter::All
                };
                demand_rates.push(DemandRate {
                    period_name: Some(period_name),
                    season,
                    rate_per_kw: rate,
                    ratchet: ratchet.clone(),
                });
            }
        }
    }

    let export_mode = parse_export_mode(&root);
    let clear_credits = matches!(
        export_mode,
        ExportMode::FlatRate(_) | ExportMode::HourlySchedule(_) | ExportMode::None
    );
    let export_rate = ExportRate {
        mode: export_mode,
        tou_credits: if clear_credits {
            Vec::new()
        } else {
            sell_credits
        },
    };

    let fixed_charges = parse_fixed_charges(&root);

    let minimum_charge = root.get("minmonthlycharge").and_then(Value::as_f64);

    // Determine seasonal split based on whether any period is season-specific
    let has_seasonal = period_seasons
        .values()
        .any(|s| matches!(s, SeasonFilter::Summer | SeasonFilter::Winter));
    let seasonal_split = if has_seasonal {
        seasonal_split_from_months(&summer_months)
    } else {
        None
    };

    let tariff = ElectricTariff {
        name: tariff_name,
        tou_schedule,
        energy_rates,
        demand_rates,
        demand_tou_schedule,
        tiered_rates,
        export_rate,
        fixed_charges,
        minimum_charge,
        minimum_charge_excludes_export: true,
        billing_cycle: BillingCycle::Monthly,
        seasonal_split,
        demand_window_minutes: 15,
        rtp_schedule: None,
        cpp_config: None,
        ev_tou_period_name: None,
    };

    tariff.validate().map_err(|e| UrdbParseError {
        message: format!("parsed tariff failed validation: {e}"),
    })?;

    Ok(tariff)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAT_RATE_JSON: &str = include_str!("../../../tests/fixtures/urdb/flat_rate.json");
    const PGE_TOU_C_JSON: &str = include_str!("../../../tests/fixtures/urdb/pge_e_tou_c.json");

    #[test]
    fn urdb_flat_rate_parses() {
        let tariff = parse(FLAT_RATE_JSON).unwrap();
        assert_eq!(tariff.name.as_deref(), Some("Flat Rate Test"));
        assert_eq!(tariff.tou_schedule.len(), 1);
        assert_eq!(tariff.tou_schedule[0].name, "period_0");
        assert_eq!(tariff.tou_schedule[0].season, SeasonFilter::All);

        // Single period covers all 24 hours
        let windows = &tariff.tou_schedule[0].schedule;
        assert!(!windows.is_empty());
        // Should have a single window covering 0:00-24:00 for Any day
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].day, DayFilter::Any);
        assert_eq!(windows[0].start_minute, 0);
        assert_eq!(windows[0].end_minute, 1440);

        assert_eq!(tariff.energy_rates.len(), 1);
        assert!((tariff.energy_rates[0].rate_per_kwh - 0.12).abs() < 1e-6);
        assert_eq!(tariff.fixed_charges.monthly_usd, 10.0);
    }

    #[test]
    fn urdb_pge_tou_c_parses() {
        let tariff = parse(PGE_TOU_C_JSON).unwrap();
        assert_eq!(tariff.name.as_deref(), Some("PG&E E-TOU-C"));
        // 3 periods: off-peak(0), partial-peak(1), on-peak(2)
        assert_eq!(tariff.tou_schedule.len(), 3);
        assert_eq!(tariff.energy_rates.len(), 3);

        let find_rate = |name: &str| {
            tariff
                .energy_rates
                .iter()
                .find(|r| r.period_name == name)
                .unwrap()
                .rate_per_kwh
        };
        assert!((find_rate("period_0") - 0.12).abs() < 1e-6);
        assert!((find_rate("period_1") - 0.18).abs() < 1e-6);
        assert!((find_rate("period_2") - 0.35).abs() < 1e-6);
    }

    #[test]
    fn urdb_pge_tou_c_demand_rates() {
        let tariff = parse(PGE_TOU_C_JSON).unwrap();
        // Should have demand rates (flat + TOU)
        assert!(!tariff.demand_rates.is_empty());

        // Check ratchet config
        let with_ratchet = tariff
            .demand_rates
            .iter()
            .find(|d| d.ratchet.is_some())
            .expect("should have at least one demand rate with ratchet");
        let ratchet = with_ratchet.ratchet.as_ref().unwrap();
        assert!((ratchet.minimum_fraction - 0.85).abs() < 1e-6);
        assert_eq!(ratchet.lookback_months, 11);

        // Demand rate value
        assert!(with_ratchet.rate_per_kw > 0.0);
    }

    #[test]
    fn urdb_pge_tou_c_seasonal_split() {
        let tariff = parse(PGE_TOU_C_JSON).unwrap();

        // period_2 (on-peak) should be summer-only (months 6-9)
        let on_peak = tariff
            .tou_schedule
            .iter()
            .find(|p| p.name == "period_2")
            .expect("period_2 should exist");
        assert_eq!(on_peak.season, SeasonFilter::Summer);

        // period_1 (partial-peak) should be winter-only (months 1-5, 10-12)
        let partial_peak = tariff
            .tou_schedule
            .iter()
            .find(|p| p.name == "period_1")
            .expect("period_1 should exist");
        assert_eq!(partial_peak.season, SeasonFilter::Winter);

        // period_0 (off-peak) should be all-season
        let off_peak = tariff
            .tou_schedule
            .iter()
            .find(|p| p.name == "period_0")
            .expect("period_0 should exist");
        assert_eq!(off_peak.season, SeasonFilter::All);

        // Seasonal split should be set
        assert!(tariff.seasonal_split.is_some());
        let split = tariff.seasonal_split.unwrap();
        assert_eq!(split.summer_start_month, 6);
        assert_eq!(split.summer_end_month, 9);
    }

    #[test]
    fn urdb_missing_energy_structure_errors() {
        let json = r#"{
            "energyweekdayschedule": [[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]],
            "energyweekendschedule": [[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]]
        }"#;
        let result = parse(json);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.message.contains("energyratestructure"),
            "error should name the missing field: {}",
            err.message
        );
    }

    #[test]
    fn urdb_missing_optional_fields_ok() {
        // Minimal valid JSON: only required fields
        let json = r#"{
            "energyweekdayschedule": [[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]],
            "energyweekendschedule": [[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]],
            "energyratestructure": [[{"rate": 0.10}]]
        }"#;
        let tariff = parse(json).unwrap();
        assert!(tariff.demand_rates.is_empty());
        assert_eq!(tariff.export_rate.mode, ExportMode::None);
        assert_eq!(tariff.fixed_charges.monthly_usd, 0.0);
        assert!(tariff.minimum_charge.is_none());
        assert!(tariff.name.is_none());
    }

    #[test]
    fn urdb_unknown_fields_tolerated() {
        let json = r#"{
            "energyweekdayschedule": [[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]],
            "energyweekendschedule": [[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]],
            "energyratestructure": [[{"rate": 0.10}]],
            "totally_unknown_field": "should be ignored",
            "another_unknown": 42
        }"#;
        let tariff = parse(json);
        assert!(tariff.is_ok());
    }

    fn minimal_schedule_row() -> &'static str {
        "[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]"
    }

    fn minimal_valid_json(overrides: &str) -> String {
        let row = minimal_schedule_row();
        let sched =
            format!("[{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]");
        if overrides.is_empty() {
            format!(
                r#"{{"energyweekdayschedule":{sched},"energyweekendschedule":{sched},"energyratestructure":[[{{"rate":0.10}}]]}}"#,
            )
        } else {
            format!(
                r#"{{"energyweekdayschedule":{sched},"energyweekendschedule":{sched},"energyratestructure":[[{{"rate":0.10}}]],{overrides}}}"#,
            )
        }
    }

    #[test]
    fn urdb_schedule_wrong_month_count() {
        let row = minimal_schedule_row();
        // 11 months instead of 12
        let bad_sched =
            format!("[{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]");
        let good_sched =
            format!("[{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]");
        let json = format!(
            r#"{{"energyweekdayschedule":{bad_sched},"energyweekendschedule":{good_sched},"energyratestructure":[[{{"rate":0.10}}]]}}"#,
        );
        let err = parse(&json).unwrap_err();
        assert!(
            err.message.contains("12"),
            "should mention 12 months: {}",
            err.message
        );
    }

    #[test]
    fn urdb_schedule_wrong_hour_count() {
        let row = minimal_schedule_row();
        let bad_row = "[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]"; // 23 hours
        // Month 0 has 23 hours
        let bad_sched = format!(
            "[{bad_row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]"
        );
        let good_sched =
            format!("[{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]");
        let json = format!(
            r#"{{"energyweekdayschedule":{bad_sched},"energyweekendschedule":{good_sched},"energyratestructure":[[{{"rate":0.10}}]]}}"#,
        );
        let err = parse(&json).unwrap_err();
        assert!(
            err.message.contains("24"),
            "should mention 24 hours: {}",
            err.message
        );
    }

    #[test]
    fn urdb_schedule_non_integer_value() {
        let row = minimal_schedule_row();
        let bad_row = "[0.5,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]";
        let bad_sched = format!(
            "[{bad_row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]"
        );
        let good_sched =
            format!("[{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]");
        let json = format!(
            r#"{{"energyweekdayschedule":{bad_sched},"energyweekendschedule":{good_sched},"energyratestructure":[[{{"rate":0.10}}]]}}"#,
        );
        let err = parse(&json).unwrap_err();
        assert!(
            err.message.contains("not an integer"),
            "should mention non-integer: {}",
            err.message
        );
    }

    #[test]
    fn urdb_tier_missing_rate_defaults_zero() {
        let row = minimal_schedule_row();
        let sched =
            format!("[{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row},{row}]");
        let json = format!(
            r#"{{"energyweekdayschedule":{sched},"energyweekendschedule":{sched},"energyratestructure":[[{{"adj":0.0}}]]}}"#,
        );
        let tariff = parse(&json).unwrap();
        assert!((tariff.energy_rates[0].rate_per_kwh).abs() < 1e-10);
    }

    #[test]
    fn urdb_fixed_charge_daily_units() {
        let json = minimal_valid_json(r#""fixedchargefirstmeter":1.50,"fixedchargeunits":"$/day""#);
        let tariff = parse(&json).unwrap();
        assert!((tariff.fixed_charges.daily_usd - 1.50).abs() < 1e-10);
        assert!((tariff.fixed_charges.monthly_usd).abs() < 1e-10);
    }

    #[test]
    fn urdb_fixed_charge_both_meters_same_units_summed() {
        let json = minimal_valid_json(
            r#""fixedchargefirstmeter":5.00,"fixedchargesecondmeter":3.00,"fixedchargeunits":"$/month""#,
        );
        let tariff = parse(&json).unwrap();
        assert!(
            (tariff.fixed_charges.monthly_usd - 8.0).abs() < 1e-10,
            "both meters should be summed: {}",
            tariff.fixed_charges.monthly_usd
        );
        assert!(
            (tariff.fixed_charges.daily_usd).abs() < 1e-10,
            "daily should be 0 for monthly units"
        );
    }

    #[test]
    fn urdb_fixed_charge_both_meters_different_units_both_fields_populated() {
        let json = minimal_valid_json(
            r#""fixedchargefirstmeter":10.00,"fixedchargeunits":"$/month","fixedchargesecondmeter":0.50,"fixedchargesecondmeterunits":"$/day""#,
        );
        let tariff = parse(&json).unwrap();
        assert!(
            (tariff.fixed_charges.monthly_usd - 10.0).abs() < 1e-10,
            "first meter monthly: {}",
            tariff.fixed_charges.monthly_usd
        );
        assert!(
            (tariff.fixed_charges.daily_usd - 0.50).abs() < 1e-10,
            "second meter daily: {}",
            tariff.fixed_charges.daily_usd
        );
    }

    #[test]
    fn urdb_fixed_charge_single_meter_still_works() {
        let json =
            minimal_valid_json(r#""fixedchargefirstmeter":10.00,"fixedchargeunits":"$/month""#);
        let tariff = parse(&json).unwrap();
        assert!(
            (tariff.fixed_charges.monthly_usd - 10.0).abs() < 1e-10,
            "single meter monthly: {}",
            tariff.fixed_charges.monthly_usd
        );
        assert!(
            (tariff.fixed_charges.daily_usd).abs() < 1e-10,
            "daily should be 0"
        );
    }

    #[test]
    fn urdb_fixed_charge_seasonal_fields_detected() {
        let json = minimal_valid_json(
            r#""fixedchargefirstmeter":10.00,"fixedchargefirstmetergroup":[[{"rate":10.0}]]"#,
        );
        let tariff = parse(&json).unwrap();
        assert!(
            (tariff.fixed_charges.monthly_usd - 10.0).abs() < 1e-10,
            "still parses flat charge even with seasonal field"
        );
    }

    #[test]
    fn urdb_export_mode_unknown_dgrules() {
        let json = minimal_valid_json(r#""dgrules":"Unknown Mode""#);
        let tariff = parse(&json).unwrap();
        assert_eq!(tariff.export_rate.mode, ExportMode::None);
    }

    #[test]
    fn urdb_export_mode_net_billing() {
        let json = minimal_valid_json(r#""dgrules":"Net Billing Instantaneous""#);
        let tariff = parse(&json).unwrap();
        assert_eq!(tariff.export_rate.mode, ExportMode::NetBilling);
    }

    #[test]
    fn urdb_rate_values_match_source() {
        let tariff = parse(PGE_TOU_C_JSON).unwrap();

        // Spot-check specific rate values against the fixture
        let off_peak = tariff
            .energy_rates
            .iter()
            .find(|r| r.period_name == "period_0")
            .unwrap();
        assert!(
            (off_peak.rate_per_kwh - 0.12).abs() < 0.001,
            "off-peak rate should be 0.12, got {}",
            off_peak.rate_per_kwh
        );

        let partial_peak = tariff
            .energy_rates
            .iter()
            .find(|r| r.period_name == "period_1")
            .unwrap();
        assert!(
            (partial_peak.rate_per_kwh - 0.18).abs() < 0.001,
            "partial-peak rate should be 0.18, got {}",
            partial_peak.rate_per_kwh
        );

        let on_peak = tariff
            .energy_rates
            .iter()
            .find(|r| r.period_name == "period_2")
            .unwrap();
        assert!(
            (on_peak.rate_per_kwh - 0.35).abs() < 0.001,
            "on-peak rate should be 0.35, got {}",
            on_peak.rate_per_kwh
        );

        // Fixed charges
        assert!(
            (tariff.fixed_charges.monthly_usd - 10.0).abs() < 0.001,
            "fixed charge should be 10.0"
        );

        // Minimum charge
        assert_eq!(tariff.minimum_charge, Some(10.0));

        // Export mode
        assert_eq!(tariff.export_rate.mode, ExportMode::NetMetering);
    }

    #[test]
    fn demand_rate_sums_generation_transmission_distribution_components() {
        let json = minimal_valid_json(
            r#""flatdemandstructure":[[{"rate":5.0,"genrate":2.0,"transrate":1.0,"distrate":0.5}]]"#,
        );
        let tariff = parse(&json).unwrap();
        assert_eq!(tariff.demand_rates.len(), 1);
        // rate=5.0 + genrate=2.0 + transrate=1.0 + distrate=0.5 = 8.5
        assert!((tariff.demand_rates[0].rate_per_kw - 8.5).abs() < 1e-6);
    }

    #[test]
    fn demand_rate_with_only_rate_and_adj_still_parses() {
        let json = minimal_valid_json(r#""flatdemandstructure":[[{"rate":7.5,"adj":2.5}]]"#);
        let tariff = parse(&json).unwrap();
        assert_eq!(tariff.demand_rates.len(), 1);
        // rate=7.5 + adj=2.5 = 10.0, no component fields present
        assert!((tariff.demand_rates[0].rate_per_kw - 10.0).abs() < 1e-6);
    }

    #[test]
    fn flat_demand_season_from_explicit_label_not_array_position() {
        // Winter-first, Summer-second ordering. Energy schedule has summer
        // months June-September (period 2 only in months 6-9).
        let json = r#"{
            "energyweekdayschedule": [
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,2,2,2,2,2,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,2,2,2,2,2,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,2,2,2,2,2,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,2,2,2,2,2,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
            ],
            "energyweekendschedule": [
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
            ],
            "energyratestructure": [[{"rate":0.10}],[{"rate":0.10}],[{"rate":0.25}]],
            "flatdemandstructure": [
                [{"rate":8.0,"season":"Winter"}],
                [{"rate":15.0,"season":"Summer"}]
            ]
        }"#;
        let tariff = parse(json).unwrap();

        assert_eq!(tariff.demand_rates.len(), 2, "should have two demand rates");

        // The entry with season label "Winter" should be Winter, regardless
        // of being at array position 0.
        let winter_rate = tariff
            .demand_rates
            .iter()
            .find(|d| d.season == SeasonFilter::Winter && (d.rate_per_kw - 8.0).abs() < 1e-6)
            .expect("should have winter demand rate at ~8.0");
        assert_eq!(winter_rate.season, SeasonFilter::Winter);

        // The entry with season label "Summer" should be Summer.
        let summer_rate = tariff
            .demand_rates
            .iter()
            .find(|d| d.season == SeasonFilter::Summer && (d.rate_per_kw - 15.0).abs() < 1e-6)
            .expect("should have summer demand rate at ~15.0");
        assert_eq!(summer_rate.season, SeasonFilter::Summer);
    }

    #[test]
    fn flat_demand_three_entries_without_labels_assigns_all_to_extra() {
        let json = minimal_valid_json(
            r#""flatdemandstructure":[[{"rate":12.0}],[{"rate":8.0}],[{"rate":5.0}]]"#,
        );
        let tariff = parse(&json).unwrap();

        assert_eq!(
            tariff.demand_rates.len(),
            3,
            "should have three demand rates"
        );

        // Position 0 = Summer (position heuristic), 1 = Winter, 2 = All (extra)
        let seasons: Vec<SeasonFilter> = tariff.demand_rates.iter().map(|d| d.season).collect();
        assert_eq!(seasons[0], SeasonFilter::Summer, "entry 0 should be Summer");
        assert_eq!(seasons[1], SeasonFilter::Winter, "entry 1 should be Winter");
        assert_eq!(
            seasons[2],
            SeasonFilter::All,
            "entry 2 (extra) should be All"
        );
    }

    #[test]
    fn flat_demand_two_entries_without_labels_uses_position_heuristic() {
        let json = minimal_valid_json(r#""flatdemandstructure":[[{"rate":15.0}],[{"rate":8.0}]]"#);
        let tariff = parse(&json).unwrap();

        assert_eq!(tariff.demand_rates.len(), 2, "should have two demand rates");

        // Without explicit labels, position heuristic applies: 0 = Summer, 1 = Winter
        let seasons: Vec<SeasonFilter> = tariff.demand_rates.iter().map(|d| d.season).collect();
        assert_eq!(
            seasons[0],
            SeasonFilter::Summer,
            "entry 0 should be Summer (position heuristic)"
        );
        assert_eq!(
            seasons[1],
            SeasonFilter::Winter,
            "entry 1 should be Winter (position heuristic)"
        );
    }
}
