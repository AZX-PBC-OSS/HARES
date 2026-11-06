//! The Normative Annex A3 software acceptance harness: the ASHRAE
//! 140-2023 acceptance criteria as their own explicitly selected
//! comparison mode beside the informative Annex B8 example bands.
//!
//! The contract (the addendum's Annex A3): a software demonstrates
//! acceptance when a sufficient number of its results fall within the
//! A3.1 ranges for each test group, counted per the A3.2 minimum-case
//! rule. Two result forms apply to the Section 7 fabric groups: the
//! single-value results (annual heating and sensible cooling loads and
//! their case sensitivities, Tables A3-1 and A3-4, both bounds
//! inclusive) and the time-series results (the February 1 and July 14
//! hourly zone loads compared to the representative curves by the
//! Pearson correlation coefficient above its limit and the root mean
//! square error below its limit, Tables A3-2, A3-3, A3-5, A3-6).
//!
//! The minimum-case rule (A3.2) counts the group's range cases: the
//! low-mass group passes when at least 30 of its 34 range cases are
//! within range, the high-mass group at least 25 of 28. A submission
//! that did not produce a result for a range case cannot claim the
//! group: the rule's count is over the group's full set (the standard's
//! A3.3 requires the complete normative output forms), so this harness
//! REFUSES to issue a conformance claim from a partial submission and
//! records the counts instead. The refusal is the enforcement of the
//! rule, and the group's physics verdict is a record: printed in full
//! with numbers on every run, never an assertion to tune.
//!
//! This module owns no production physics and reads no B8 comparison
//! state: the runs are the fixtures' own, the pinned tables are the
//! addendum's (`data`), and the B8 example bands appear only in the
//! printed side-by-side for the record.

mod data;
mod metric;

use std::collections::BTreeMap;

use data::{
    A3Date, A3Metric, ADDENDUM_A_SHA256, ADDENDUM_A_URL, FABRIC_MINIMUM_CASE_RULES,
    MinimumCaseRule, SingleValueRange, TimeSeriesLimits,
};
use metric::pearson;
use metric::rmse;

use super::cases::{BestestCase, core_cases, extended_cases};
use super::reference_bands::BestestMetric as B8Metric;
use hares_core::Dwelling;

// The wall-clock field accessors the bucket rule reads (hour, minute,
// second, and the date's month and day).
use chrono::{Datelike, Timelike};

/// The two dates' order used by the per-run hourly slots.
const DATES: [A3Date; 2] = [A3Date::February1, A3Date::July14];

/// One case's annual simulation: the annual loads and, when the
/// simulated calendar carries the date, the two dates' hourly zone
/// loads. All loads are kWh; the hourly loads are signed with heating
/// positive and cooling negative (the representative curves'
/// convention).
#[derive(Debug, Clone)]
struct A3Run {
    case_id: &'static str,
    heating_kwh: f64,
    cooling_kwh: f64,
    /// The 24 hour-ending buckets for each date, `None` when the run
    /// did not simulate that date.
    hourly: [Option<[f64; 24]>; 2],
    /// The steps matched to each date: 24 at the fixtures' hourly
    /// timestep, one per hour-ending bucket.
    hourly_steps: [usize; 2],
}

impl A3Run {
    fn load(&self, metric: A3Metric) -> f64 {
        match metric {
            A3Metric::AnnualHeatingKwh => self.heating_kwh,
            A3Metric::AnnualCoolingKwh => self.cooling_kwh,
        }
    }
}

/// One case's annual run at the fixture's own timestep: the annual
/// loads accumulate every step's delivered HVAC energy, and each step
/// whose wall-clock date is February 1 or July 14 lands its signed load
/// in the date's hour-ending bucket.
///
/// The bucket rule: a step's load lands in the hour the step ends in,
/// the same hour-ending convention the environment's weather addressing
/// follows (the record of the hour the step ends in). The hour-ending
/// label is also the representative curves' own: Hour 1 is the interval
/// midnight to 01:00 local standard time, and the curves' Case 640
/// morning pick-up spike (3.06 kWh at Hour 8) is the 07:00-08:00
/// interval the case's 23:00-07:00 setback schedule recovers into. At
/// the fixtures' 3600 s timestep starting on the hour, each bucket
/// holds exactly one step.
fn run_a3_case(case: &BestestCase) -> A3Run {
    let mut dwelling =
        Dwelling::from_toml_config_with_reports(&case.fixture_path(), Some(Vec::new()))
            .unwrap_or_else(|err| panic!("failed to load BESTEST case {}: {err}", case.id));
    let total_steps = dwelling.clock.total_steps();
    let dt_hours = case.timestep_seconds as f64 / 3600.0;
    let mut run = A3Run {
        case_id: case.id,
        heating_kwh: 0.0,
        cooling_kwh: 0.0,
        hourly: [None, None],
        hourly_steps: [0; 2],
    };
    for _ in 0..total_steps {
        let step = dwelling
            .step()
            .unwrap_or_else(|err| panic!("simulation failed for BESTEST case {}: {err}", case.id));
        run.heating_kwh += step.hvac_heating_w / 1000.0 * dt_hours;
        run.cooling_kwh += step.hvac_cooling_w / 1000.0 * dt_hours;

        // The step's wall-clock fields: the naive local standard-time
        // stamp at the step's interval start. The end-of-step second
        // within the day selects the hour-ending bucket: ending on the
        // hour H gives bucket H, and a step ending at midnight ends
        // into bucket 24 of the step's own date.
        let ts = &step.timestamp;
        let end_secs = i64::from(ts.hour()) * 3600
            + i64::from(ts.minute()) * 60
            + i64::from(ts.second())
            + case.timestep_seconds;
        let bucket = end_secs / 3600;
        assert!(
            (1..=24).contains(&bucket),
            "case {}: the timestep {} s spans more than the hourly sampling \
             can bucket (end second {end_secs})",
            case.id,
            case.timestep_seconds
        );
        let date = ts.date();
        for ((slot, steps), a3date) in run
            .hourly
            .iter_mut()
            .zip(run.hourly_steps.iter_mut())
            .zip(DATES)
        {
            let (month, day) = a3date.month_day();
            if date.month() == month && date.day() == day {
                let buckets = slot.get_or_insert([0.0; 24]);
                // Signed zone load: heating positive, cooling negative.
                buckets[(bucket - 1) as usize] +=
                    (step.hvac_heating_w - step.hvac_cooling_w) / 1000.0 * dt_hours;
                *steps += 1;
            }
        }
    }
    run
}

/// A3.1's single-value rule: within when the result is greater than or
/// equal to the lower limit and less than or equal to the upper limit.
/// Both bounds are inclusive; the boundary pins prove the inclusivity.
fn within_single_value(range: &SingleValueRange, value_kwh: f64) -> bool {
    value_kwh >= range.min_kwh && value_kwh <= range.max_kwh
}

/// A3.1's time-series rule: within when the Pearson correlation
/// coefficient is ABOVE the limit AND the root mean square error is
/// BELOW the limit. Both comparisons are strict; equality on either
/// bound is outside (the boundary pins prove the strictness).
fn within_time_series(
    limits: &TimeSeriesLimits,
    date: A3Date,
    pearson_r: f64,
    rmse_kwh: f64,
) -> bool {
    let above_pearson = pearson_r > limits.min_pearson;
    let below_rmse = match date {
        A3Date::February1 => rmse_kwh < limits.max_rmse_feb1_kwh,
        A3Date::July14 => match limits.max_rmse_jul14_kwh {
            Some(max) => rmse_kwh < max,
            // A case with no July 14 limit has no July 14 range: the
            // pinned tables pair every July curve column with a limit,
            // so this arm is unreachable for them.
            None => false,
        },
    };
    above_pearson && below_rmse
}

/// One single-value range case's outcome: the sensitivity (or absolute)
/// value when every operand case ran, and the range comparison.
#[derive(Debug, Clone)]
struct SingleValueRecord {
    range: &'static SingleValueRange,
    /// The computed value; `None` when an operand case has no annual
    /// run (the range case has no result in this submission).
    value_kwh: Option<f64>,
    within: bool,
}

/// One date's time-series comparison for one case.
#[derive(Debug, Clone)]
struct TimeSeriesRecord {
    limits: &'static TimeSeriesLimits,
    date: A3Date,
    comparison: Option<TimeSeriesComparison>,
    within: bool,
}

#[derive(Debug, Clone, Copy)]
struct TimeSeriesComparison {
    pearson_r: f64,
    rmse_kwh: f64,
}

/// The A3.2 verdict for one test group. The refusal is structural: a
/// submission missing range-case results cannot claim the group
/// regardless of how the evaluated results fall.
#[derive(Debug, Clone, PartialEq)]
enum Disposition {
    /// Every range case evaluated; the within count meets or exceeds
    /// the minimum.
    Conformant,
    /// Every range case evaluated; the within count is under the
    /// minimum.
    NonConformant,
    /// The submission is incomplete: range cases with no result exist,
    /// so no conformance claim may be issued. The missing count is the
    /// record.
    Refused { missing: usize },
}

impl std::fmt::Display for Disposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conformant => write!(f, "CONFORMANT"),
            Self::NonConformant => write!(f, "NON-CONFORMANT"),
            Self::Refused { missing } => write!(
                f,
                "REFUSED (incomplete submission: {missing} range cases without a result)"
            ),
        }
    }
}

/// A3.2's count rule over one group's range cases. `within` may not
/// exceed `evaluated`; the refusal dominates the count: a partial
/// submission never conforms even when every evaluated result is within
/// range.
fn group_disposition(total: usize, minimum: usize, evaluated: usize, within: usize) -> Disposition {
    assert!(
        within <= evaluated,
        "within-range count {within} exceeds the evaluated count {evaluated}"
    );
    if evaluated < total {
        return Disposition::Refused {
            missing: total - evaluated,
        };
    }
    if within >= minimum {
        Disposition::Conformant
    } else {
        Disposition::NonConformant
    }
}

/// One group's full verdict: the rule, the counts, the disposition, and
/// the records (the printed table's rows).
struct GroupVerdict {
    rule: &'static MinimumCaseRule,
    disposition: Disposition,
    evaluated: usize,
    within_count: usize,
    single_value: Vec<SingleValueRecord>,
    time_series: Vec<TimeSeriesRecord>,
}

fn evaluate_single_value_group(
    runs: &BTreeMap<&'static str, A3Run>,
    ranges: &'static [SingleValueRange],
) -> Vec<SingleValueRecord> {
    ranges
        .iter()
        .map(|range| {
            let minuend = runs.get(range.minuend);
            let subtrahend = runs.get(range.subtrahend);
            // An absolute row's value is the case's own load; a
            // sensitivity row's value is the minuend case's load minus
            // the subtrahend case's load.
            let value_kwh = if range.minuend == range.subtrahend {
                minuend.map(|a| a.load(range.metric))
            } else {
                minuend
                    .zip(subtrahend)
                    .map(|(a, b)| a.load(range.metric) - b.load(range.metric))
            };
            let within = value_kwh.is_some_and(|v| within_single_value(range, v));
            SingleValueRecord {
                range,
                value_kwh,
                within,
            }
        })
        .collect()
}

fn evaluate_time_series_group(
    runs: &BTreeMap<&'static str, A3Run>,
    limits: &'static [TimeSeriesLimits],
) -> Vec<TimeSeriesRecord> {
    let mut records = Vec::new();
    for row in limits {
        for date in DATES {
            // The curve column and the date's limit exist together: the
            // addendum prints them as one criterion. The tables' own
            // n/a (Case 640 and Case 940 have no July 14 columns) makes
            // the pair absent on both sides, and the pair is skipped.
            let Some(curve) = data::representative_curve(row.case_id, date) else {
                continue;
            };
            let has_limit = match date {
                A3Date::February1 => Some(row.max_rmse_feb1_kwh),
                A3Date::July14 => row.max_rmse_jul14_kwh,
            };
            if has_limit.is_none() {
                panic!(
                    "the pinned A3 tables are inconsistent: case {} carries a {} \
                     representative curve but no {} RMSE limit",
                    row.case_id,
                    date.name(),
                    date.name()
                );
            };
            let comparison = runs.get(row.case_id).and_then(|run| {
                let slot = match date {
                    A3Date::February1 => run.hourly[0],
                    A3Date::July14 => run.hourly[1],
                };
                slot.map(|day| {
                    let pearson_r = pearson(&day, &curve).unwrap_or_else(|err| {
                        panic!(
                            "case {} {} hourly comparison: {err}",
                            row.case_id,
                            date.name()
                        )
                    });
                    let rmse_kwh = rmse(&day, &curve).unwrap_or_else(|err| {
                        panic!(
                            "case {} {} hourly comparison: {err}",
                            row.case_id,
                            date.name()
                        )
                    });
                    TimeSeriesComparison {
                        pearson_r,
                        rmse_kwh,
                    }
                })
            });
            let within =
                comparison.is_some_and(|c| within_time_series(row, date, c.pearson_r, c.rmse_kwh));
            records.push(TimeSeriesRecord {
                limits: row,
                date,
                comparison,
                within,
            });
        }
    }
    records
}

fn evaluate_group(
    rule: &'static MinimumCaseRule,
    runs: &BTreeMap<&'static str, A3Run>,
    ranges: &'static [SingleValueRange],
    limits: &'static [TimeSeriesLimits],
) -> GroupVerdict {
    let single_value = evaluate_single_value_group(runs, ranges);
    let time_series = evaluate_time_series_group(runs, limits);
    let evaluated = single_value
        .iter()
        .filter(|r| r.value_kwh.is_some())
        .count()
        + time_series
            .iter()
            .filter(|r| r.comparison.is_some())
            .count();
    let within_count = single_value.iter().filter(|r| r.within).count()
        + time_series.iter().filter(|r| r.within).count();
    let disposition = group_disposition(
        rule.range_cases,
        rule.minimum_within,
        evaluated,
        within_count,
    );
    GroupVerdict {
        rule,
        disposition,
        evaluated,
        within_count,
        single_value,
        time_series,
    }
}

/// The B8 example band's column name where the B8 metric is one of the
/// annual loads (the side-by-side print's reference column).
fn b8_band_column(metric: &B8Metric) -> Option<&'static str> {
    match metric {
        B8Metric::AnnualHeatingLoadKwh | B8Metric::AnnualHeatingEnergyKwh => {
            Some("annual_heating_kwh")
        }
        B8Metric::AnnualCoolingLoadKwh => Some("annual_cooling_kwh"),
        B8Metric::PeakZoneTempC | B8Metric::MinZoneTempC => None,
    }
}

/// The verdict's full printed table: every range case with its value
/// and comparison, the group counts, and the disposition. This is the
/// A3 record: every number the submission carries, printed on every
/// run, with the B8 example bands beside the absolute rows for
/// reference.
fn verdict_report(verdict: &GroupVerdict) -> String {
    let mut out = String::new();
    let rule = verdict.rule;
    out.push_str(&format!(
        "[A3] group \"{}\" (tables {}): range_cases={} minimum_within={} \
         evaluated={} within={} disposition={}\n",
        rule.group,
        rule.tables,
        rule.range_cases,
        rule.minimum_within,
        verdict.evaluated,
        verdict.within_count,
        verdict.disposition
    ));
    for record in &verdict.single_value {
        let range = record.range;
        // The B8 example bands are absolute per-case rows; a
        // sensitivity row has no B8 counterpart and prints none.
        let b8_note = if range.minuend == range.subtrahend {
            super::reference_bands::core_reference_bands(range.minuend)
                .iter()
                .find(|band| b8_band_column(&band.metric) == Some(range.metric.column_name()))
                .map(|band| format!(" | B8 example band [{:.0}, {:.0}] kWh", band.min, band.max))
                .unwrap_or_default()
        } else {
            String::new()
        };
        match record.value_kwh {
            Some(value) => out.push_str(&format!(
                "[A3]   {:>22} {:>19}: value {:+.3} kWh, A3 [{:.0}, {:.0}] kWh: {}{}\n",
                range.range_case,
                range.metric.column_name(),
                value,
                range.min_kwh,
                range.max_kwh,
                if record.within { "WITHIN" } else { "OUTSIDE" },
                b8_note
            )),
            None => out.push_str(&format!(
                "[A3]   {:>22} {:>19}: NO RESULT (no annual fixture for case {}), \
                 A3 [{:.0}, {:.0}] kWh\n",
                range.range_case,
                range.metric.column_name(),
                range.minuend,
                range.min_kwh,
                range.max_kwh
            )),
        }
    }
    for record in &verdict.time_series {
        let row = record.limits;
        let rmse_limit = match record.date {
            A3Date::February1 => row.max_rmse_feb1_kwh,
            // The pinned tables pair every printed curve column with a
            // limit; a record for a July column carries one.
            A3Date::July14 => row
                .max_rmse_jul14_kwh
                .expect("every printed July curve column carries a July limit"),
        };
        match record.comparison {
            Some(c) => out.push_str(&format!(
                "[A3]   Case {} {:>10}: pearson {:+.4} (limit >{:.3}), rmse {:.4} kWh \
                 (limit <{:.4} kWh): {}\n",
                row.case_id,
                record.date.name(),
                c.pearson_r,
                row.min_pearson,
                c.rmse_kwh,
                rmse_limit,
                if record.within { "WITHIN" } else { "OUTSIDE" }
            )),
            None => out.push_str(&format!(
                "[A3]   Case {} {:>10}: NO RESULT (no annual fixture), limits \
                 [pearson >{:.3}, rmse <{:.4} kWh]\n",
                row.case_id,
                record.date.name(),
                row.min_pearson,
                rmse_limit
            )),
        }
    }
    out
}

/// The six annual Section 7 fixtures the repository carries: the
/// low-mass 600, 640, 650 and the high-mass 900, 920, 950. The Section
/// 7 group's remaining cases (610, 620, 630, 660, 670, 680, 685, 695,
/// 910, 930, 940, 960, 980, 985, 995) have no annual fixtures; the 610
/// and 620 files are two-day construction stubs, not the standard's
/// annual cases.
fn section7_annual_runs() -> BTreeMap<&'static str, A3Run> {
    let wanted = ["600", "640", "650", "900", "920", "950"];
    let registry: Vec<_> = core_cases().into_iter().chain(extended_cases()).collect();
    let mut runs = BTreeMap::new();
    for id in wanted {
        let case = registry
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("case {id} is in the BESTEST case registry"));
        let run = run_a3_case(case);
        runs.insert(run.case_id, run);
    }
    runs
}

/// The A3 record run: both Section 7 fabric groups evaluated over the
/// six annual fixtures, the full table printed, and the structural
/// contract asserted.
///
/// The assertions are structural, never the physics: the two dates the
/// standard requires resolve to exactly 24 hourly loads for every curve
/// column whose fixture exists; every range case whose operand fixtures
/// exist carries a result; the group dispositions are the refusal the
/// minimum-case rule demands from an incomplete submission (15 Section
/// 7 cases have no annual fixtures, so 26 of the low-mass group's 34
/// and 21 of the high-mass group's 28 range cases have no result and no
/// conformance claim may be issued). The physics numbers (each within
/// or outside, each Pearson and RMSE) are the record printed above the
/// assertions: an A3 failure is a finding with numbers, never a tuning
/// target, and no B8 result substitutes for an A3 one.
#[test]
fn annex_a3_section7_group_verdicts_are_records() {
    let runs = section7_annual_runs();
    assert_eq!(runs.len(), 6, "the submission runs the six annual fixtures");

    let low = evaluate_group(
        &FABRIC_MINIMUM_CASE_RULES[0],
        &runs,
        data::LOW_MASS_SINGLE_VALUE_RANGES,
        data::LOW_MASS_TIME_SERIES_LIMITS,
    );
    let high = evaluate_group(
        &FABRIC_MINIMUM_CASE_RULES[1],
        &runs,
        data::HIGH_MASS_SINGLE_VALUE_RANGES,
        data::HIGH_MASS_TIME_SERIES_LIMITS,
    );

    eprintln!("[A3] source: {ADDENDUM_A_URL} (sha256 {ADDENDUM_A_SHA256})");
    eprint!("{}", verdict_report(&low));
    eprint!("{}", verdict_report(&high));

    for verdict in [&low, &high] {
        // The calendar contract: for every curve column whose case ran,
        // the date's extraction resolves exactly 24 hourly loads from
        // exactly 24 steps (the fixtures' hourly timestep, one step per
        // bucket).
        for record in &verdict.time_series {
            if runs.contains_key(record.limits.case_id) {
                let run = &runs[record.limits.case_id];
                let (slot, steps) = match record.date {
                    A3Date::February1 => (&run.hourly[0], run.hourly_steps[0]),
                    A3Date::July14 => (&run.hourly[1], run.hourly_steps[1]),
                };
                assert!(
                    slot.is_some() && steps == 24,
                    "case {}: the {} hourly extraction must resolve 24 hourly \
                     loads from 24 steps for the standard's date contract",
                    record.limits.case_id,
                    record.date.name()
                );
            }
        }
        // Evaluation completeness follows the runs: a range case is
        // evaluated exactly when its operand fixtures exist.
        for record in &verdict.single_value {
            let operands_present = runs.contains_key(record.range.minuend)
                && runs.contains_key(record.range.subtrahend);
            assert_eq!(
                record.value_kwh.is_some(),
                operands_present,
                "range case {}: evaluation completeness must follow the operand \
                 fixtures' presence",
                record.range.range_case
            );
        }
        // The minimum-case rule: with range cases without results
        // present, the disposition is the refusal, whatever the
        // evaluated results did.
        let incomplete = verdict.evaluated < verdict.rule.range_cases;
        assert_eq!(
            incomplete,
            matches!(verdict.disposition, Disposition::Refused { .. }),
            "group {}: the refusal must follow submission incompleteness",
            verdict.rule.group
        );
        if incomplete {
            eprintln!(
                "[A3] verdict: {} for group \"{}\": the submission carries no result \
                 for {} Section 7 range cases, so no A3 conformance claim may be \
                 issued; the evaluated results are the record above",
                verdict.disposition,
                verdict.rule.group,
                verdict.rule.range_cases - verdict.evaluated
            );
        }
    }

    // The submission's own shape: the six fixtures leave 26 low-mass
    // and 21 high-mass range cases without results, so both groups
    // refuse. Adding the missing annual fixtures moves this pin
    // deliberately, with that row's record.
    assert_eq!(
        low.disposition,
        Disposition::Refused { missing: 26 },
        "the low-mass group's submission is incomplete (no annual fixtures for \
         610, 620, 630, 660, 670, 680, 685, 695 and their curve columns): the \
         A3 verdict must refuse, not conform"
    );
    assert_eq!(
        high.disposition,
        Disposition::Refused { missing: 21 },
        "the high-mass group's submission is incomplete (no annual fixtures for \
         910, 930, 940, 960, 980, 985, 995 and their curve columns): the A3 \
         verdict must refuse, not conform"
    );
}

#[cfg(test)]
mod tests {
    use super::data::{self, A3Date, A3Metric};
    use super::{Disposition, group_disposition, within_single_value, within_time_series};

    /// A3.1's single-value bounds are inclusive on both ends: "greater
    /// than or equal to the lower limit and less than or equal to the
    /// upper limit". The 1e-9 offsets in the rejections are about 2000
    /// times the f64 spacing at these magnitudes (ulp ~9e-13 at 4000),
    /// so the rejected values sit genuinely outside the range; the
    /// accepted values are the bounds themselves.
    #[test]
    fn a3_single_value_bounds_are_inclusive() {
        let range = &data::LOW_MASS_SINGLE_VALUE_RANGES[0]; // Case 600 heating
        assert_eq!((range.min_kwh, range.max_kwh), (3750.0, 4980.0));
        assert!(
            within_single_value(range, 3750.0),
            "the lower bound is inside"
        );
        assert!(
            within_single_value(range, 4980.0),
            "the upper bound is inside"
        );
        assert!(within_single_value(range, 4000.0));
        assert!(!within_single_value(range, 3750.0 - 1e-9));
        assert!(!within_single_value(range, 4980.0 + 1e-9));
    }

    /// A3.1's time-series bounds are strict on both metrics: within
    /// when the Pearson coefficient is ABOVE the limit and the RMSE is
    /// BELOW it. Equality on either bound is outside. The 1e-9 offsets
    /// are vast against the f64 spacing at these magnitudes (ulp
    /// ~1e-16 at 0.879), so the accepted and rejected values are
    /// distinct reals, not rounding artifacts.
    #[test]
    fn a3_time_series_bounds_are_strictly_exclusive() {
        let row = &data::LOW_MASS_TIME_SERIES_LIMITS[0]; // Case 600
        assert_eq!(row.min_pearson, 0.879);
        assert_eq!(row.max_rmse_feb1_kwh, 0.4194);
        // Equality on either bound: outside.
        assert!(!within_time_series(row, A3Date::February1, 0.879, 0.1));
        assert!(!within_time_series(row, A3Date::February1, 0.99, 0.4194));
        // Strictly inside on both: within.
        assert!(within_time_series(
            row,
            A3Date::February1,
            0.879 + 1e-9,
            0.4194 - 1e-9
        ));
        // The July limit rides its own date's bound.
        let jul_limit = row.max_rmse_jul14_kwh.unwrap();
        assert_eq!(jul_limit, 0.1921);
        assert!(!within_time_series(row, A3Date::July14, 0.99, jul_limit));
        assert!(within_time_series(
            row,
            A3Date::July14,
            0.879 + 1e-9,
            jul_limit - 1e-9
        ));
    }

    /// The pinned tables' shape is the addendum's: the applicable
    /// single-value cells and the curve columns make up exactly the
    /// range-case counts Table A3-14 prints, the n/a cells are
    /// recorded, and the minimum-case rule carries the printed counts.
    #[test]
    fn a3_tables_match_the_addendum_counts() {
        // Low mass: 21 applicable single-value cells (the printed 11
        // rows' two load columns less the one n/a) + 13 curve columns
        // (7 February 1 + 6 July 14) = 34; minimum 30.
        assert_eq!(data::LOW_MASS_SINGLE_VALUE_RANGES.len(), 21);
        assert_eq!(
            data::LOW_MASS_FEB1_CASES.len() + data::LOW_MASS_JUL14_CASES.len(),
            13
        );
        assert_eq!(data::LOW_MASS_TIME_SERIES_LIMITS.len(), 7);
        // High mass: 19 applicable single-value cells (10 rows less the
        // one n/a) + 9 curve columns (5 + 4) = 28; minimum 25.
        assert_eq!(data::HIGH_MASS_SINGLE_VALUE_RANGES.len(), 19);
        assert_eq!(
            data::HIGH_MASS_FEB1_CASES.len() + data::HIGH_MASS_JUL14_CASES.len(),
            9
        );
        assert_eq!(data::HIGH_MASS_TIME_SERIES_LIMITS.len(), 5);
        // The n/a cells: the two heating rows the addendum prints n/a.
        assert_eq!(data::SINGLE_VALUE_NOT_APPLICABLE.len(), 2);
        assert!(
            data::SINGLE_VALUE_NOT_APPLICABLE
                .iter()
                .all(|(_, metric)| *metric == A3Metric::AnnualHeatingKwh)
        );
        // The rules carry exactly the printed totals, and the counts
        // re-derive from the pinned tables.
        for rule in data::FABRIC_MINIMUM_CASE_RULES {
            let (ranges, feb, jul, printed) = match rule.group {
                "Thermal Fabric Low Mass" => (
                    data::LOW_MASS_SINGLE_VALUE_RANGES.len(),
                    data::LOW_MASS_FEB1_CASES.len(),
                    data::LOW_MASS_JUL14_CASES.len(),
                    (34usize, 30usize),
                ),
                "Thermal Fabric High Mass" => (
                    data::HIGH_MASS_SINGLE_VALUE_RANGES.len(),
                    data::HIGH_MASS_FEB1_CASES.len(),
                    data::HIGH_MASS_JUL14_CASES.len(),
                    (28, 25),
                ),
                other => panic!("unknown group {other}"),
            };
            assert_eq!((rule.range_cases, rule.minimum_within), printed);
            assert_eq!(
                ranges + feb + jul,
                rule.range_cases,
                "group {}: the pinned tables must re-derive the addendum's \
                 printed range-case count",
                rule.group
            );
        }
    }

    /// The curve columns' transcription pins: each column's sum and
    /// corner cells, computed from the hash-verified addendum PDF. The
    /// pinned sums are the exact rational sums of 24 two-decimal values;
    /// f64 accumulation introduces rounding below 1e-10 in magnitude
    /// (24 additions of values under 60, each with relative error under
    /// 2^-52), while a single transcribed digit moves a sum by at least
    /// 0.01: the tolerance cannot mask a transcription error.
    #[test]
    fn a3_curve_columns_sum_to_the_transcription_pins() {
        let pins: &[(&str, A3Date, f64)] = &[
            ("600", A3Date::February1, 5.98),
            ("640", A3Date::February1, -1.45),
            ("660", A3Date::February1, 16.82),
            ("670", A3Date::February1, 14.08),
            ("680", A3Date::February1, -14.28),
            ("685", A3Date::February1, 0.64),
            ("695", A3Date::February1, -18.67),
            ("600", A3Date::July14, -18.86),
            ("660", A3Date::July14, -13.92),
            ("670", A3Date::July14, -20.98),
            ("680", A3Date::July14, -16.91),
            ("685", A3Date::July14, -29.77),
            ("695", A3Date::July14, -25.69),
            ("900", A3Date::February1, 11.61),
            ("940", A3Date::February1, 4.94),
            ("980", A3Date::February1, -5.66),
            ("985", A3Date::February1, 4.30),
            ("995", A3Date::February1, -14.13),
            ("900", A3Date::July14, -14.02),
            ("980", A3Date::July14, -14.87),
            ("985", A3Date::July14, -28.46),
            ("995", A3Date::July14, -25.25),
        ];
        for (case_id, date, expected) in pins {
            let curve = data::representative_curve(case_id, *date)
                .unwrap_or_else(|| panic!("curve {case_id} {} exists", date.name()));
            let sum: f64 = curve.iter().sum();
            assert!(
                (sum - expected).abs() < 1e-6,
                "curve {case_id} {}: the column sums to {sum} against the \
                 transcription pin {expected}",
                date.name()
            );
        }
        // Corner cells: the printed layout's first, peak and last hour
        // of the shared columns.
        let feb600 = data::representative_curve("600", A3Date::February1).unwrap();
        assert_eq!((feb600[0], feb600[13], feb600[23]), (2.08, -4.67, 2.44));
        let jul600 = data::representative_curve("600", A3Date::July14).unwrap();
        assert_eq!((jul600[0], jul600[12], jul600[23]), (0.0, -2.70, 0.0));
        let feb900 = data::representative_curve("900", A3Date::February1).unwrap();
        assert_eq!((feb900[0], feb900[23]), (0.98, 0.29));
        let jul900 = data::representative_curve("900", A3Date::July14).unwrap();
        assert_eq!((jul900[0], jul900[12], jul900[23]), (-0.10, -1.22, -0.37));
        // The addendum's own n/a: no July 14 curve for 640 or 940.
        assert_eq!(data::representative_curve("640", A3Date::July14), None);
        assert_eq!(data::representative_curve("940", A3Date::July14), None);
    }

    /// The time-series limits' transcription pins: the Pearson floor is
    /// 0.879 on every row, the February RMSE ceilings carry the
    /// printed values, and the July n/a rows are the addendum's own.
    #[test]
    fn a3_time_series_limits_match_the_addendum() {
        for row in data::LOW_MASS_TIME_SERIES_LIMITS {
            assert_eq!(row.min_pearson, 0.879);
        }
        for row in data::HIGH_MASS_TIME_SERIES_LIMITS {
            assert_eq!(row.min_pearson, 0.879);
        }
        let pins: &[(&str, f64, Option<f64>)] = &[
            ("600", 0.4194, Some(0.1921)),
            ("640", 0.6559, None),
            ("660", 0.3087, Some(0.1697)),
            ("670", 0.7881, Some(0.3018)),
            ("680", 0.3990, Some(0.2097)),
            ("685", 0.4643, Some(0.3218)),
            ("695", 0.2069, Some(0.2001)),
            ("900", 0.3716, Some(0.0967)),
            ("940", 0.4298, None),
            ("980", 0.2969, Some(0.0986)),
            ("985", 0.4166, Some(0.2053)),
            ("995", 0.2944, Some(0.2468)),
        ];
        for (case_id, feb, jul) in pins {
            let row = data::LOW_MASS_TIME_SERIES_LIMITS
                .iter()
                .chain(data::HIGH_MASS_TIME_SERIES_LIMITS.iter())
                .find(|row| row.case_id == *case_id)
                .unwrap_or_else(|| panic!("limits for case {case_id} exist"));
            assert_eq!(
                row.max_rmse_feb1_kwh, *feb,
                "case {case_id} February RMSE limit"
            );
            assert_eq!(
                row.max_rmse_jul14_kwh, *jul,
                "case {case_id} July RMSE limit"
            );
        }
    }

    /// The A3.2 count rule: a complete group conforms at exactly the
    /// minimum within count and fails one below it.
    #[test]
    fn a3_minimum_case_rule_conforms_at_the_minimum_count() {
        assert_eq!(
            group_disposition(34, 30, 34, 30),
            Disposition::Conformant,
            "the rule is 'meets or exceeds': 30 of 34 within conforms"
        );
        assert_eq!(group_disposition(34, 30, 34, 33), Disposition::Conformant);
        assert_eq!(group_disposition(28, 25, 28, 25), Disposition::Conformant);
        assert_eq!(
            group_disposition(34, 30, 34, 29),
            Disposition::NonConformant
        );
        assert_eq!(
            group_disposition(28, 25, 28, 24),
            Disposition::NonConformant
        );
    }

    /// The minimum-case rule's refusal: a submission missing range-case
    /// results refuses even when every evaluated result falls within
    /// range. A partial run is not a menu: no conformance claim may be
    /// issued from it.
    #[test]
    fn a3_partial_run_refuses_even_when_every_evaluated_result_is_within_range() {
        assert_eq!(
            group_disposition(34, 30, 8, 8),
            Disposition::Refused { missing: 26 },
            "8 of 34 evaluated, all 8 within: still refused"
        );
        assert_eq!(
            group_disposition(28, 25, 7, 7),
            Disposition::Refused { missing: 21 }
        );
        // One evaluated result short of complete: refused.
        assert_eq!(
            group_disposition(34, 30, 33, 33),
            Disposition::Refused { missing: 1 }
        );
    }

    /// The absolute range rows read the case's own load: the Case 600
    /// heating row's value is Case 600's annual heating load, not a
    /// self-subtracted sensitivity (a self-subtraction reads 0.000 and
    /// reports OUTSIDE against the row's true value). The witness runs
    /// the evaluator on a hand-pinned load and reads the record off
    /// the row.
    #[test]
    fn a3_absolute_rows_read_the_cases_own_load() {
        use super::{A3Run, evaluate_single_value_group};
        let runs = [(
            "600",
            A3Run {
                case_id: "600",
                heating_kwh: 4000.0,
                cooling_kwh: 6000.0,
                hourly: [None, None],
                hourly_steps: [0; 2],
            },
        )]
        .into_iter()
        .collect();
        let records = evaluate_single_value_group(&runs, data::LOW_MASS_SINGLE_VALUE_RANGES);
        let heating = records
            .iter()
            .find(|r| {
                r.range.range_case == "Case 600" && r.range.metric == A3Metric::AnnualHeatingKwh
            })
            .expect("the Case 600 heating row is pinned");
        assert_eq!(
            heating.value_kwh,
            Some(4000.0),
            "the absolute row reads the case's own load"
        );
        assert!(heating.within, "4000 is inside the pinned [3750, 4980]");
        let cooling = records
            .iter()
            .find(|r| {
                r.range.range_case == "Case 600" && r.range.metric == A3Metric::AnnualCoolingKwh
            })
            .expect("the Case 600 cooling row is pinned");
        assert_eq!(cooling.value_kwh, Some(6000.0));
        assert!(cooling.within, "6000 is inside the pinned [5000, 6830]");
        // The sensitivity rows stay differences: with Case 600 as the
        // only run, the 640 - 600 row has no result.
        let sensitivity = records
            .iter()
            .find(|r| r.range.range_case == "Case 640 - Case 600")
            .expect("the 640 - 600 row is pinned");
        assert_eq!(sensitivity.value_kwh, None);
        assert!(!sensitivity.within);
    }
}
