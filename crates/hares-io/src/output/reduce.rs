//! Period reduction: step rows reduced into minute to annual reporting
//! periods, each variable by its declared aggregation.
//!
//! The reducer is fed per step from a report's projection of the row plan
//! ([`PeriodReducer::push_row`]) or per record batch
//! ([`PeriodReducer::push_batch`]); both feeds perform the same operations
//! in the same order, so they produce the same rows on bits. Rows are
//! labelled at the nominal period start in the naive local wall clock the
//! row's own timestamp carries; a closed period carries the steps that fell
//! in it and, per rule, the reduced value and the count of valid steps.

use arrow::array::{Array, Float64Array, RecordBatch, StringArray};
use arrow::datatypes::DataType;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike};
use hares_types::{Aggregation, VariableKind};
use thiserror::Error;

use super::catalogue::VariableDef;
use super::report::Frequency;

/// The local calendar a reduction buckets onto: the home's own site offset
/// today. The period-origin offset a fleet's `Alignment::Instant` needs
/// arrives with the fleet's calendar entry, which extends this struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeriodCalendar;

/// The over-time reduction rule of one variable, from its
/// [`VariableDef::aggregation`]. `enum_codes` is the label count of an
/// `Enum` variable, so `Mode` can count per code; it is `None` for every
/// other kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRule {
    /// The declared over-time rule.
    pub aggregation: Aggregation,
    /// The label count of an `Enum` variable (`Mode` counts per code).
    pub enum_codes: Option<usize>,
}

impl TimeRule {
    /// The rule of one catalogue variable: its declared aggregation and,
    /// for an `Enum` kind, the label count the mode counts per.
    #[must_use]
    pub fn from_variable(def: &VariableDef) -> Self {
        Self {
            aggregation: def.aggregation,
            enum_codes: match &def.kind {
                VariableKind::Enum { labels } => Some(labels.len()),
                _ => None,
            },
        }
    }
}

/// One closed period, borrowed from the reducer's buffers, so emitting
/// allocates nothing per period beyond what the receiving side chooses.
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodRow<'a> {
    /// The nominal period start, naive local milliseconds.
    pub start_local_ms: i64,
    /// The steps that fell in the period, valid or not.
    pub step_count: u32,
    /// The reduced value per rule, in rule order. A period whose valid
    /// step count is zero for a rule carries `0.0` here and is null: the
    /// matching `valid_steps` entry, not the value, decides.
    pub values: &'a [f64],
    /// The valid (non-null) step count per rule, in rule order. Zero means
    /// the period's value for that rule is null.
    pub valid_steps: &'a [u32],
}

/// Why a period reduction refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReduceError {
    /// The `Timestep` frequency is the step stream itself: there is no
    /// period to reduce into.
    #[error("the Timestep frequency takes no period reduction")]
    Timestep,
    /// `Minutes(n)` requires `n` to divide the hour: the frequency's own
    /// rule, named with the value that broke it.
    #[error("Minutes({0}) is not a valid reporting frequency: n must divide the hour (60)")]
    MinutesNotDividingHour(u32),
    /// A `Mode` rule counts per code, so it needs the enum's label count.
    #[error("a Mode rule needs enum_codes set to the enum's label count")]
    ModeWithoutCodes,
    /// A batch fed the reducer carries fewer columns than the rules read.
    #[error(
        "the batch carries {got} columns but the reducer reads {expected}: \
         the Time column plus one column per rule"
    )]
    ColumnCount { expected: usize, got: usize },
    /// A batch column is not the type the feed reads.
    #[error("batch column '{name}' must be {expected}")]
    ColumnType {
        /// The offending column's name.
        name: String,
        /// What the column must be.
        expected: &'static str,
    },
    /// A batch `Time` value does not parse as an RFC 3339 timestamp at a
    /// fixed offset.
    #[error("batch Time value '{0}' does not parse as an RFC 3339 timestamp")]
    TimeValue(String),
}

/// One rule's accumulation over the open period.
#[derive(Debug)]
struct RuleState {
    aggregation: Aggregation,
    /// The step-order sum for `Sum` and `Mean` (`Mean` divides once at
    /// emit); the held value for `Last`.
    acc: f64,
    /// Per-code counts for `Mode` (sized to the enum's label count).
    counts: Vec<u32>,
    /// The count of valid steps so far in the period.
    valid_steps: u32,
}

impl RuleState {
    fn new(rule: &TimeRule) -> Self {
        let counts = rule
            .enum_codes
            .map(|codes| vec![0_u32; codes])
            .unwrap_or_default();
        Self {
            aggregation: rule.aggregation,
            acc: 0.0,
            counts,
            valid_steps: 0,
        }
    }

    fn reset(&mut self) {
        self.acc = 0.0;
        self.counts.fill(0);
        self.valid_steps = 0;
    }

    /// Adds one valid step value, in step order. A `Mode` code outside the
    /// declared labels contributes nothing: the catalogue declares a label
    /// at every code a variable can publish, so an out-of-range code is a
    /// defect, and the reducer drops it by the same rule the batch feed
    /// applies to an unreadable value (no count, no valid step).
    fn add(&mut self, value: f64) {
        match self.aggregation {
            Aggregation::Sum | Aggregation::Mean => self.acc += value,
            Aggregation::Last => self.acc = value,
            Aggregation::Mode => {
                let codes = self.counts.len();
                let in_range = value.is_finite()
                    && value >= 0.0
                    && value.fract() == 0.0
                    && (value as usize) < codes;
                if in_range {
                    self.counts[value as usize] += 1;
                } else {
                    return;
                }
            }
        }
        self.valid_steps += 1;
    }

    /// The period's reduced value. A period with no valid step is null;
    /// the value slot carries `0.0` and the valid count decides.
    fn reduce(&self) -> f64 {
        match self.aggregation {
            Aggregation::Sum | Aggregation::Last => self.acc,
            Aggregation::Mean => {
                if self.valid_steps > 0 {
                    self.acc / f64::from(self.valid_steps)
                } else {
                    0.0
                }
            }
            Aggregation::Mode => self.mode_winner() as f64,
        }
    }

    /// The most frequent code, ties to the lowest code: a strict maximum
    /// keeps the first (lowest) code on a tie.
    fn mode_winner(&self) -> usize {
        let mut best = 0_usize;
        let mut best_count = self.counts.first().copied().unwrap_or(0);
        for (code, &count) in self.counts.iter().enumerate().skip(1) {
            if count > best_count {
                best = code;
                best_count = count;
            }
        }
        best
    }
}

/// Reduces the step stream of one report into calendar periods: minutes to
/// run period, each variable by its declared aggregation.
///
/// The reducer buckets on the naive local wall clock the row's own
/// timestamp carries (the run's site offset), never on the row plan's
/// per-row field, which is stamped in UTC epoch seconds. Because the
/// validated period is a whole multiple of the timestep, every period
/// boundary coincides with a step interval end: a step belongs to the
/// period its interval completes in, which is the period the code floors
/// the step's local time into.
///
/// The reducer carries no per-step allocation: per-rule state reuses fixed
/// buffers, and the emitted [`PeriodRow`] borrows them.
#[derive(Debug)]
pub struct PeriodReducer {
    frequency: Frequency,
    rules: Vec<RuleState>,
    /// The open period's nominal start, naive local milliseconds. The first
    /// fed row opens the run period's single period; every later row joins
    /// it.
    open_start_ms: Option<i64>,
    /// The open period's step count, valid or not.
    step_count: u32,
    /// The reduced value buffer the emit borrows.
    out_values: Vec<f64>,
    /// The valid-step buffer the emit borrows.
    out_valid_steps: Vec<u32>,
}

impl PeriodReducer {
    /// Builds a reducer for one non-`Timestep` report.
    ///
    /// # Errors
    ///
    /// [`ReduceError::Timestep`] for the `Timestep` frequency,
    /// [`ReduceError::MinutesNotDividingHour`] for a `Minutes(n)` whose `n`
    /// does not divide the hour, and [`ReduceError::ModeWithoutCodes`] for
    /// a `Mode` rule without the enum's label count.
    pub fn new(
        frequency: Frequency,
        calendar: PeriodCalendar,
        rules: Vec<TimeRule>,
    ) -> Result<Self, ReduceError> {
        let _ = calendar;
        match frequency {
            Frequency::Timestep => return Err(ReduceError::Timestep),
            Frequency::Minutes(n) if 60 % n.get() != 0 => {
                return Err(ReduceError::MinutesNotDividingHour(n.get()));
            }
            _ => {}
        }
        if rules
            .iter()
            .any(|rule| rule.aggregation == Aggregation::Mode && rule.enum_codes.is_none())
        {
            return Err(ReduceError::ModeWithoutCodes);
        }
        Ok(Self {
            frequency,
            rules: rules.iter().map(RuleState::new).collect(),
            open_start_ms: None,
            step_count: 0,
            out_values: vec![0.0; rules.len()],
            out_valid_steps: vec![0_u32; rules.len()],
        })
    }

    /// The frequency the reducer reduces onto.
    #[must_use]
    pub fn frequency(&self) -> Frequency {
        self.frequency
    }

    /// Feeds one step: the row's naive local milliseconds (the run's site
    /// offset applied to the step instant, the time the row's own timestamp
    /// column carries), the step's values in rule order, and their
    /// validity. Closed periods emit through `emit` in order.
    pub fn push_row(
        &mut self,
        local_ms: i64,
        values: &[f64],
        valid: &[bool],
        emit: &mut dyn FnMut(PeriodRow<'_>),
    ) {
        debug_assert_eq!(values.len(), self.rules.len());
        debug_assert_eq!(valid.len(), self.rules.len());
        self.advance(local_ms, emit);
        for (rule, (value, &is_valid)) in self.rules.iter_mut().zip(values.iter().zip(valid)) {
            if is_valid {
                rule.add(*value);
            }
        }
    }

    /// Feeds a record batch: the `Time` column first (Utf8, RFC 3339 at the
    /// run's fixed offset), then one Float64 column per rule, in rule
    /// order. A batch may carry extra columns beyond the rules, which are
    /// ignored; the batch's `Time` values are parsed to naive local
    /// milliseconds, so both feeds bucket on the same local milliseconds.
    ///
    /// # Errors
    ///
    /// [`ReduceError::ColumnCount`] when the batch has fewer columns than
    /// the rules read, [`ReduceError::ColumnType`] when the `Time` column
    /// or a rule column is of the wrong type, and
    /// [`ReduceError::TimeValue`] when a `Time` value does not parse.
    pub fn push_batch(
        &mut self,
        batch: &RecordBatch,
        emit: &mut dyn FnMut(PeriodRow<'_>),
    ) -> Result<(), ReduceError> {
        let expected = self.rules.len() + 1;
        if batch.num_columns() < expected {
            return Err(ReduceError::ColumnCount {
                expected,
                got: batch.num_columns(),
            });
        }
        let time_field = batch.schema_ref().field(0).clone();
        let type_error = |name: &str| ReduceError::ColumnType {
            name: name.to_string(),
            expected: "the Utf8 Time column first",
        };
        if time_field.name() != "Time" || time_field.data_type() != &DataType::Utf8 {
            return Err(type_error(time_field.name()));
        }
        let Some(time_values) = batch.column(0).as_any().downcast_ref::<StringArray>() else {
            return Err(type_error(time_field.name()));
        };
        let mut rule_columns = Vec::with_capacity(self.rules.len());
        for index in 0..self.rules.len() {
            let field = batch.schema_ref().field(index + 1);
            let Some(column) = batch
                .column(index + 1)
                .as_any()
                .downcast_ref::<Float64Array>()
            else {
                return Err(ReduceError::ColumnType {
                    name: field.name().clone(),
                    expected: "a Float64 value column in rule order",
                });
            };
            rule_columns.push(column);
        }
        for row in 0..batch.num_rows() {
            let time_value = time_values.value(row);
            let parsed = DateTime::parse_from_rfc3339(time_value)
                .map_err(|_| ReduceError::TimeValue(time_value.to_string()))?;
            let local_ms = parsed.naive_local().and_utc().timestamp_millis();
            self.advance(local_ms, emit);
            for (rule, column) in self.rules.iter_mut().zip(rule_columns.iter().copied()) {
                if !column.is_null(row) {
                    rule.add(column.value(row));
                }
            }
        }
        Ok(())
    }

    /// Closes the open partial period, if any, and consumes the reducer.
    pub fn finish(mut self, emit: &mut dyn FnMut(PeriodRow<'_>)) {
        self.close(emit);
    }

    /// Advances the period bookkeeping for a row fed at `local_ms`: closes
    /// the open period and opens the row's period when the row falls past
    /// it, then counts the step.
    fn advance(&mut self, local_ms: i64, emit: &mut dyn FnMut(PeriodRow<'_>)) {
        let start = self.period_start(local_ms);
        if self.open_start_ms != Some(start) {
            self.close(emit);
            self.open_start_ms = Some(start);
        }
        self.step_count += 1;
    }

    /// The nominal period start of a row fed at `local_ms`, naive local
    /// milliseconds: the wall-clock floor for `Minutes`, `Hourly` and
    /// `Daily`, the calendar month or year start for `Monthly` and
    /// `Annual`, and the run's first row for `RunPeriod`.
    fn period_start(&self, local_ms: i64) -> i64 {
        if matches!(self.frequency, Frequency::RunPeriod) {
            return self.open_start_ms.unwrap_or(local_ms);
        }
        let naive = naive_local(local_ms);
        let start = match self.frequency {
            Frequency::Timestep | Frequency::RunPeriod => unreachable!("handled above"),
            Frequency::Minutes(every) => {
                let every = i64::from(every.get());
                let minute = i64::from(naive.minute());
                naive
                    .with_minute((minute / every * every) as u32)
                    .and_then(|n| n.with_second(0))
                    .and_then(|n| n.with_nanosecond(0))
            }
            Frequency::Hourly => naive
                .with_minute(0)
                .and_then(|n| n.with_second(0))
                .and_then(|n| n.with_nanosecond(0)),
            Frequency::Daily => NaiveDate::from_ymd_opt(naive.year(), naive.month(), naive.day())
                .and_then(|date| date.and_hms_opt(0, 0, 0)),
            Frequency::Monthly => NaiveDate::from_ymd_opt(naive.year(), naive.month(), 1)
                .and_then(|date| date.and_hms_opt(0, 0, 0)),
            Frequency::Annual => NaiveDate::from_ymd_opt(naive.year(), 1, 1)
                .and_then(|date| date.and_hms_opt(0, 0, 0)),
        };
        start
            .unwrap_or_else(|| panic!("period start of {local_ms} ms out of the chrono range"))
            .and_utc()
            .timestamp_millis()
    }

    /// Emits the open period and resets the per-rule state.
    fn close(&mut self, emit: &mut dyn FnMut(PeriodRow<'_>)) {
        let Some(start) = self.open_start_ms else {
            return;
        };
        for (rule, value) in self.rules.iter().zip(self.out_values.iter_mut()) {
            *value = rule.reduce();
        }
        for (rule, valid) in self.rules.iter().zip(self.out_valid_steps.iter_mut()) {
            *valid = rule.valid_steps;
        }
        self.rules.iter_mut().for_each(RuleState::reset);
        self.open_start_ms = None;
        let step_count = std::mem::take(&mut self.step_count);
        emit(PeriodRow {
            start_local_ms: start,
            step_count,
            values: &self.out_values,
            valid_steps: &self.out_valid_steps,
        });
    }
}

/// Reads naive local wall-clock milliseconds back as a `NaiveDateTime`:
/// the millisecond scale is the wall clock, so the UTC reading of the
/// converted value is the naive local time.
fn naive_local(local_ms: i64) -> NaiveDateTime {
    DateTime::from_timestamp_millis(local_ms)
        .unwrap_or_else(|| panic!("local milliseconds {local_ms} out of the chrono range"))
        .naive_utc()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;

    use arrow::array::{Array, Float64Array, RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use chrono::{Datelike, FixedOffset, TimeZone, Timelike};
    use hares_types::Aggregation;

    use super::{PeriodCalendar, PeriodReducer, PeriodRow, ReduceError, TimeRule, naive_local};
    use crate::output::report::Frequency;

    const STEP_MS: i64 = 900_000;

    fn offset() -> FixedOffset {
        FixedOffset::east_opt(-7 * 3600).expect("offset")
    }

    fn offset_half_hour_east() -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 1800).expect("offset")
    }

    /// The naive local wall-clock milliseconds of a whole-second civil time
    /// at `site`.
    fn local_ms_at(site: FixedOffset, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        site.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("local time")
            .naive_local()
            .and_utc()
            .timestamp_millis()
    }

    /// A period row's Time label: the naive local period start rendered by
    /// the same function the step rows render with (chrono `%+` at the
    /// run's site offset).
    fn time_label(start_local_ms: i64, site: FixedOffset) -> String {
        let naive = naive_local(start_local_ms);
        site.with_ymd_and_hms(
            naive.year(),
            naive.month(),
            naive.day(),
            naive.hour(),
            naive.minute(),
            naive.second(),
        )
        .single()
        .expect("local time")
        .format("%+")
        .to_string()
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Collected {
        start_local_ms: i64,
        step_count: u32,
        values: Vec<f64>,
        valid_steps: Vec<u32>,
    }

    /// The sink every feed emits into: an unannotated closure coerced to
    /// the reducer's emit parameter, so the higher-ranked `PeriodRow`
    /// lifetime is inferred from the reducer's signature.
    fn sink(rows: &mut Vec<Collected>) -> impl FnMut(PeriodRow<'_>) + '_ {
        |row| {
            rows.push(Collected {
                start_local_ms: row.start_local_ms,
                step_count: row.step_count,
                values: row.values.to_vec(),
                valid_steps: row.valid_steps.to_vec(),
            });
        }
    }

    fn collect(reducer: PeriodReducer, rows: &mut Vec<Collected>) {
        reducer.finish(&mut sink(rows));
    }

    fn rule(aggregation: Aggregation) -> TimeRule {
        TimeRule {
            aggregation,
            enum_codes: None,
        }
    }

    fn mode_rule(codes: usize) -> TimeRule {
        TimeRule {
            aggregation: Aggregation::Mode,
            enum_codes: Some(codes),
        }
    }

    fn minutes(n: u32) -> Frequency {
        Frequency::Minutes(NonZeroU32::new(n).expect("nonzero"))
    }

    /// The synthetic day at `step_ms`: from 2023-01-01T00:00 local at
    /// -07:00, a day long. Per step: the step's energy (1.0 each), the
    /// step's power (the 1-based step index), an enum code cycling 0, 1, 2,
    /// and a running total publishing the 1-based step index.
    fn synthetic_day(step_ms: i64) -> Vec<(i64, Vec<f64>)> {
        let base = local_ms_at(offset(), 2023, 1, 1, 0, 0);
        let steps = 86_400_000 / step_ms;
        (0..steps)
            .map(|i| {
                let step = i as f64;
                (
                    base + i * step_ms,
                    vec![1.0, step + 1.0, step % 3.0, step + 1.0],
                )
            })
            .collect()
    }

    fn day_rules() -> Vec<TimeRule> {
        vec![
            rule(Aggregation::Sum),
            rule(Aggregation::Mean),
            mode_rule(3),
            rule(Aggregation::Last),
        ]
    }

    fn reduce_day(frequency: Frequency, step_ms: i64, valid: &[bool; 4]) -> Vec<Collected> {
        let mut reducer =
            PeriodReducer::new(frequency, PeriodCalendar, day_rules()).expect("reducer builds");
        let mut rows = Vec::new();
        for (local_ms, values) in synthetic_day(step_ms) {
            reducer.push_row(local_ms, &values, valid, &mut sink(&mut rows));
        }
        collect(reducer, &mut rows);
        rows
    }

    #[test]
    fn minute_report_sums_and_means_over_a_synthetic_day() {
        // The entry's minutes case: a Minutes(15) report at a 300 s step,
        // three steps per quarter hour, 96 rows over the day.
        let rows = reduce_day(minutes(15), 300_000, &[true; 4]);

        assert_eq!(rows.len(), 96, "one row per quarter hour of the day");
        for (p, row) in rows.iter().enumerate() {
            let period = p as f64;
            // The energy sums: three steps of 1.0, added in step order.
            assert_eq!(row.values[0].to_bits(), 3.0_f64.to_bits(), "period {p}");
            // The power means: the period's steps carry 3p+1 .. 3p+3; the
            // mean divides the step-order sum once.
            let sum = (period * 3.0 + 1.0) + (period * 3.0 + 2.0) + (period * 3.0 + 3.0);
            assert_eq!(row.values[1].to_bits(), (sum / 3.0).to_bits(), "period {p}");
            // The enum modes: the period's three codes are the three
            // residues once each, a tie, so the lowest code wins.
            assert_eq!(row.values[2].to_bits(), 0.0_f64.to_bits(), "period {p}");
            // The running totals: the period's third step's index.
            assert_eq!(
                row.values[3].to_bits(),
                (period * 3.0 + 3.0).to_bits(),
                "period {p}"
            );
            assert_eq!(row.step_count, 3, "period {p}");
            assert_eq!(row.valid_steps, &[3, 3, 3, 3], "period {p}");
        }
        // The labels: quarter hours on the local clock, from :00 through
        // :45, the first row at local midnight.
        assert_eq!(
            time_label(rows[0].start_local_ms, offset()),
            "2023-01-01T00:00:00-07:00"
        );
        assert_eq!(
            time_label(rows[1].start_local_ms, offset()),
            "2023-01-01T00:15:00-07:00"
        );
        assert_eq!(
            time_label(rows[2].start_local_ms, offset()),
            "2023-01-01T00:30:00-07:00"
        );
        assert_eq!(
            time_label(rows[3].start_local_ms, offset()),
            "2023-01-01T00:45:00-07:00"
        );
        assert_eq!(
            time_label(rows[95].start_local_ms, offset()),
            "2023-01-01T23:45:00-07:00"
        );
    }

    #[test]
    fn hourly_report_over_the_synthetic_day_matches_hand_computation() {
        let rows = reduce_day(Frequency::Hourly, STEP_MS, &[true; 4]);

        assert_eq!(rows.len(), 24, "one row per hour of the day");
        for (h, row) in rows.iter().enumerate() {
            let hour = h as f64;
            assert_eq!(row.values[0].to_bits(), 4.0_f64.to_bits(), "hour {h}");
            let sum =
                (hour * 4.0 + 1.0) + (hour * 4.0 + 2.0) + (hour * 4.0 + 3.0) + (hour * 4.0 + 4.0);
            assert_eq!(row.values[1].to_bits(), (sum / 4.0).to_bits(), "hour {h}");
            // The hour's codes are (4h mod 3) twice and the successors once
            // each: the mode is 4h mod 3.
            assert_eq!(
                row.values[2].to_bits(),
                (((h as i64) * 4 % 3) as f64).to_bits(),
                "hour {h}"
            );
            assert_eq!(
                row.values[3].to_bits(),
                (hour * 4.0 + 4.0).to_bits(),
                "hour {h}"
            );
            assert_eq!(row.step_count, 4, "hour {h}");
        }
        assert_eq!(
            time_label(rows[0].start_local_ms, offset()),
            "2023-01-01T00:00:00-07:00"
        );
        assert_eq!(
            time_label(rows[23].start_local_ms, offset()),
            "2023-01-01T23:00:00-07:00"
        );

        // The whole day at the coarser calendar periods: one row each,
        // labelled at the calendar start, with the day's sums and the
        // running total's end-of-day value.
        for frequency in [Frequency::Daily, Frequency::Monthly, Frequency::Annual] {
            let rows = reduce_day(frequency, STEP_MS, &[true; 4]);
            assert_eq!(rows.len(), 1, "{frequency:?} covers the day in one row");
            assert_eq!(rows[0].values[0].to_bits(), 96.0_f64.to_bits());
            assert_eq!(rows[0].values[1].to_bits(), 48.5_f64.to_bits());
            // 96 steps over codes 0, 1, 2: 32 of each, a tie, so the lowest.
            assert_eq!(rows[0].values[2].to_bits(), 0.0_f64.to_bits());
            assert_eq!(rows[0].values[3].to_bits(), 96.0_f64.to_bits());
            assert_eq!(rows[0].step_count, 96);
            assert_eq!(
                time_label(rows[0].start_local_ms, offset()),
                "2023-01-01T00:00:00-07:00"
            );
        }
    }

    #[test]
    fn partial_first_and_last_period_carry_step_count() {
        // 91 steps of 900 s from 00:30 to 23:15 local, the end exclusive:
        // the first hour carries 2 steps, the last 1, every hour between 4.
        let mut reducer = PeriodReducer::new(
            Frequency::Hourly,
            PeriodCalendar,
            vec![rule(Aggregation::Sum)],
        )
        .expect("reducer builds");
        let base = local_ms_at(offset(), 2023, 1, 1, 0, 30);
        let mut rows = Vec::new();
        for i in 0..91 {
            reducer.push_row(
                base + i64::from(i) * STEP_MS,
                &[1.0],
                &[true],
                &mut sink(&mut rows),
            );
        }
        collect(reducer, &mut rows);

        assert_eq!(rows.len(), 24);
        assert_eq!(rows[0].step_count, 2, "the run starts at 00:30");
        assert_eq!(rows[0].values[0].to_bits(), 2.0_f64.to_bits());
        for row in &rows[1..23] {
            assert_eq!(row.step_count, 4);
        }
        assert_eq!(rows[23].step_count, 1, "the run ends at 23:15 exclusive");
        assert_eq!(rows[23].values[0].to_bits(), 1.0_f64.to_bits());
        // The labels are the nominal period starts; the first row's Time is
        // the period's start, not the run's first step.
        assert_eq!(
            time_label(rows[0].start_local_ms, offset()),
            "2023-01-01T00:00:00-07:00"
        );
        assert_eq!(
            time_label(rows[23].start_local_ms, offset()),
            "2023-01-01T23:00:00-07:00"
        );
    }

    #[test]
    fn calendar_periods_label_at_their_start_across_boundaries() {
        // One step at 2022-12-31T23:45, then three across 2023-01-01's
        // first quarter hour: every frequency labels at its period start.
        let times = [
            local_ms_at(offset(), 2022, 12, 31, 23, 45),
            local_ms_at(offset(), 2023, 1, 1, 0, 0),
            local_ms_at(offset(), 2023, 1, 1, 0, 15),
            local_ms_at(offset(), 2023, 1, 1, 0, 30),
        ];
        let cases: [(Frequency, Vec<&str>); 3] = [
            (
                Frequency::Daily,
                vec!["2022-12-31T00:00:00-07:00", "2023-01-01T00:00:00-07:00"],
            ),
            (
                Frequency::Monthly,
                vec!["2022-12-01T00:00:00-07:00", "2023-01-01T00:00:00-07:00"],
            ),
            (
                Frequency::Annual,
                vec!["2022-01-01T00:00:00-07:00", "2023-01-01T00:00:00-07:00"],
            ),
        ];
        for (frequency, labels) in cases {
            let mut reducer =
                PeriodReducer::new(frequency, PeriodCalendar, vec![rule(Aggregation::Sum)])
                    .expect("reducer builds");
            let mut rows = Vec::new();
            for (i, &local_ms) in times.iter().enumerate() {
                reducer.push_row(
                    local_ms,
                    &[f64::from(i as u32) + 1.0],
                    &[true],
                    &mut sink(&mut rows),
                );
            }
            collect(reducer, &mut rows);

            assert_eq!(rows.len(), 2, "{frequency:?}");
            assert_eq!(rows[0].step_count, 1, "{frequency:?}");
            assert_eq!(rows[1].step_count, 3, "{frequency:?}");
            assert_eq!(
                rows[0].values[0].to_bits(),
                1.0_f64.to_bits(),
                "{frequency:?}"
            );
            assert_eq!(
                rows[1].values[0].to_bits(),
                9.0_f64.to_bits(),
                "{frequency:?}"
            );
            assert_eq!(
                time_label(rows[0].start_local_ms, offset()),
                labels[0],
                "{frequency:?}"
            );
            assert_eq!(
                time_label(rows[1].start_local_ms, offset()),
                labels[1],
                "{frequency:?}"
            );
        }
    }

    #[test]
    fn null_cells_excluded_with_valid_count() {
        let mut reducer = PeriodReducer::new(
            Frequency::Hourly,
            PeriodCalendar,
            vec![
                rule(Aggregation::Sum),
                rule(Aggregation::Mean),
                rule(Aggregation::Last),
            ],
        )
        .expect("reducer builds");
        let base = local_ms_at(offset(), 2023, 1, 1, 0, 0);
        let mut rows = Vec::new();
        // First hour: nulls on the second and fourth steps; the reduction
        // runs over the valid steps only, in step order.
        for (i, (value, valid)) in [(1.0, true), (2.0, false), (3.0, true), (4.0, false)]
            .into_iter()
            .enumerate()
        {
            reducer.push_row(
                base + i64::try_from(i).expect("step index") * STEP_MS,
                &[value, value, value],
                &[valid, valid, valid],
                &mut sink(&mut rows),
            );
        }
        // Second hour: every step null. The period is null: the value slot
        // carries 0.0 and the valid counts are zero.
        for i in 0..4 {
            reducer.push_row(
                base + 3_600_000 + i64::from(i) * STEP_MS,
                &[0.0, 0.0, 0.0],
                &[false; 3],
                &mut sink(&mut rows),
            );
        }
        // Third hour: nulls between valid steps; `Last` holds the last
        // VALID step's value.
        for (i, (value, valid)) in [(1.0, true), (2.0, false), (3.0, false), (4.0, true)]
            .into_iter()
            .enumerate()
        {
            reducer.push_row(
                base + 7_200_000 + i64::try_from(i).expect("step index") * STEP_MS,
                &[value, value, value],
                &[valid, valid, valid],
                &mut sink(&mut rows),
            );
        }
        collect(reducer, &mut rows);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].values[0].to_bits(), 4.0_f64.to_bits());
        assert_eq!(rows[0].values[1].to_bits(), 2.0_f64.to_bits());
        assert_eq!(rows[0].values[2].to_bits(), 3.0_f64.to_bits());
        assert_eq!(rows[0].valid_steps, &[2, 2, 2]);
        assert_eq!(rows[0].step_count, 4, "null steps still count as steps");
        assert_eq!(rows[1].values, &[0.0, 0.0, 0.0], "the null period's slots");
        assert_eq!(rows[1].valid_steps, &[0, 0, 0], "the null period is marked");
        assert_eq!(rows[1].step_count, 4);
        assert_eq!(rows[2].values[0].to_bits(), 5.0_f64.to_bits());
        assert_eq!(rows[2].values[1].to_bits(), 2.5_f64.to_bits());
        assert_eq!(rows[2].values[2].to_bits(), 4.0_f64.to_bits());
        assert_eq!(rows[2].valid_steps, &[2, 2, 2]);
    }

    #[test]
    fn run_period_report_has_one_row() {
        let mut reducer = PeriodReducer::new(Frequency::RunPeriod, PeriodCalendar, day_rules())
            .expect("reducer builds");
        let mut rows = Vec::new();
        for (local_ms, values) in synthetic_day(STEP_MS) {
            reducer.push_row(local_ms, &values, &[true; 4], &mut sink(&mut rows));
        }
        assert!(
            rows.is_empty(),
            "the run period's row cannot emit before finish"
        );
        collect(reducer, &mut rows);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].step_count, 96);
        assert_eq!(rows[0].values[0].to_bits(), 96.0_f64.to_bits());
        assert_eq!(rows[0].values[1].to_bits(), 48.5_f64.to_bits());
        assert_eq!(rows[0].values[2].to_bits(), 0.0_f64.to_bits());
        assert_eq!(rows[0].values[3].to_bits(), 96.0_f64.to_bits());
        let first = local_ms_at(offset(), 2023, 1, 1, 0, 0);
        assert_eq!(rows[0].start_local_ms, first, "labelled at the run's start");

        // An empty feed finishes to no rows at all.
        let reducer =
            PeriodReducer::new(Frequency::RunPeriod, PeriodCalendar, day_rules()).expect("builds");
        let mut rows = Vec::new();
        collect(reducer, &mut rows);
        assert!(rows.is_empty());
    }

    #[test]
    fn batch_fed_reducer_matches_row_fed_reducer_on_bits() {
        let base = local_ms_at(offset(), 2023, 1, 1, 0, 0);
        let times: Vec<String> = (0..6)
            .map(|i| time_label(base + i64::from(i) * STEP_MS, offset()))
            .collect();
        let borrowed: Vec<&str> = times.iter().map(String::as_str).collect();
        // A null in the first hour's third step; a third column no rule
        // reads, to pin that columns beyond the rules are ignored. Both
        // rules read the same column: the sum and the mean over its valid
        // steps.
        let column_a = vec![Some(1.0), Some(2.0), None, Some(4.0), Some(5.0), Some(6.0)];
        let column_b = column_a.clone();
        let column_extra = vec![Some(99.0); 6];
        let batch = batch_with_columns(&borrowed, &[column_a, column_b, column_extra]);

        let rules = vec![rule(Aggregation::Sum), rule(Aggregation::Mean)];
        let mut row_fed =
            PeriodReducer::new(Frequency::Hourly, PeriodCalendar, rules.clone()).expect("builds");
        let mut batch_fed =
            PeriodReducer::new(Frequency::Hourly, PeriodCalendar, rules).expect("builds");
        let mut row_rows = Vec::new();
        let mut batch_rows = Vec::new();

        let row_values = [1.0, 2.0, 0.0, 4.0, 5.0, 6.0];
        for i in 0_i64..6 {
            let value_a = row_values[i as usize];
            let valid_a = i != 2;
            row_fed.push_row(
                base + i * STEP_MS,
                &[value_a, value_a],
                &[valid_a, valid_a],
                &mut sink(&mut row_rows),
            );
        }
        collect(row_fed, &mut row_rows);
        batch_fed
            .push_batch(&batch, &mut sink(&mut batch_rows))
            .expect("the batch feeds");
        collect(batch_fed, &mut batch_rows);

        assert_eq!(row_rows, batch_rows, "both feeds, on bits");
        assert_eq!(batch_rows.len(), 2);
        assert_eq!(batch_rows[0].values[0].to_bits(), 7.0_f64.to_bits());
        let mean: f64 = 1.0 + 2.0 + 4.0;
        assert_eq!(batch_rows[0].values[1].to_bits(), (mean / 3.0).to_bits());
        assert_eq!(batch_rows[0].valid_steps, &[3, 3]);
        assert_eq!(batch_rows[0].step_count, 4);
        assert_eq!(batch_rows[1].values[0].to_bits(), 11.0_f64.to_bits());
        assert_eq!(batch_rows[1].values[1].to_bits(), 5.5_f64.to_bits());
    }

    #[test]
    fn push_batch_rejects_column_count_type_and_time_mismatches() {
        let base = local_ms_at(offset(), 2023, 1, 1, 0, 0);
        let times: Vec<String> = (0..2)
            .map(|i| time_label(base + i64::from(i) * STEP_MS, offset()))
            .collect();
        let borrowed: Vec<&str> = times.iter().map(String::as_str).collect();
        let rules = vec![rule(Aggregation::Sum)];

        // Fewer columns than the rules read.
        let short = batch_with_columns(&borrowed, &[]);
        let mut reducer =
            PeriodReducer::new(Frequency::Hourly, PeriodCalendar, rules.clone()).expect("builds");
        let mut rows = Vec::new();
        let err = reducer
            .push_batch(&short, &mut sink(&mut rows))
            .expect_err("a Time-only batch cannot feed one rule");
        assert!(
            matches!(
                err,
                ReduceError::ColumnCount {
                    expected: 2,
                    got: 1
                }
            ),
            "got: {err:?}"
        );

        // A Float64 column where the Utf8 Time column belongs.
        let float_time = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("Time", DataType::Float64, false),
                Field::new("c0", DataType::Float64, true),
            ])),
            vec![
                Arc::new(Float64Array::from(vec![0.0; 2])),
                Arc::new(Float64Array::from(vec![Some(1.0); 2])),
            ],
        )
        .expect("batch");
        let err = reducer
            .push_batch(&float_time, &mut sink(&mut rows))
            .expect_err("a Float64 Time column is a type mismatch");
        assert!(
            matches!(err, ReduceError::ColumnType { ref name, .. } if name == "Time"),
            "got: {err:?}"
        );

        // A rule column that is not Float64.
        let float_rule = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("Time", DataType::Utf8, false),
                Field::new("c0", DataType::Float32, true),
            ])),
            vec![
                Arc::new(StringArray::from(borrowed.clone())),
                Arc::new(arrow::array::Float32Array::from(vec![1.0; 2])),
            ],
        )
        .expect("batch");
        let err = reducer
            .push_batch(&float_rule, &mut sink(&mut rows))
            .expect_err("a Float32 rule column is a type mismatch");
        assert!(
            matches!(err, ReduceError::ColumnType { ref name, .. } if name == "c0"),
            "got: {err:?}"
        );

        // A Time value that does not parse.
        let unparseable =
            batch_with_columns(&["not a timestamp", "also not"], &[vec![Some(1.0); 2]]);
        let err = reducer
            .push_batch(&unparseable, &mut sink(&mut rows))
            .expect_err("an unparseable Time value is an error");
        assert!(
            matches!(err, ReduceError::TimeValue(ref value) if value == "not a timestamp"),
            "got: {err:?}"
        );
    }

    #[test]
    fn timestep_and_bad_minutes_are_reducer_errors() {
        let err = PeriodReducer::new(Frequency::Timestep, PeriodCalendar, vec![])
            .expect_err("the Timestep frequency takes no reduction");
        assert!(matches!(err, ReduceError::Timestep), "got: {err:?}");

        for n in [7_u32, 90] {
            let err = PeriodReducer::new(minutes(n), PeriodCalendar, vec![])
                .expect_err("minutes that do not divide the hour are refused");
            assert!(
                matches!(err, ReduceError::MinutesNotDividingHour(value) if value == n),
                "got: {err:?}"
            );
        }

        let err = PeriodReducer::new(
            Frequency::Hourly,
            PeriodCalendar,
            vec![TimeRule {
                aggregation: Aggregation::Mode,
                enum_codes: None,
            }],
        )
        .expect_err("a Mode rule without the label count is refused");
        assert!(matches!(err, ReduceError::ModeWithoutCodes), "got: {err:?}");
    }

    #[test]
    fn periods_start_on_the_local_clock_at_a_half_hour_offset() {
        let site = offset_half_hour_east();
        let base = local_ms_at(site, 2023, 1, 1, 0, 7);
        let mut reducer =
            PeriodReducer::new(minutes(15), PeriodCalendar, vec![rule(Aggregation::Sum)])
                .expect("reducer builds");
        let mut rows = Vec::new();
        for i in 0..4 {
            reducer.push_row(
                base + i64::from(i) * STEP_MS,
                &[1.0],
                &[true],
                &mut sink(&mut rows),
            );
        }
        collect(reducer, &mut rows);

        assert_eq!(rows.len(), 4);
        let labels: Vec<String> = rows
            .iter()
            .map(|row| time_label(row.start_local_ms, site))
            .collect();
        assert_eq!(
            labels,
            vec![
                "2023-01-01T00:00:00+05:30",
                "2023-01-01T00:15:00+05:30",
                "2023-01-01T00:30:00+05:30",
                "2023-01-01T00:45:00+05:30",
            ]
        );

        // The hour bucket at the same offset, and the day boundary on the
        // local wall clock: a step at 23:30 and one at 23:45 fall on Jan 1,
        // two past local midnight fall on Jan 2.
        let mut reducer = PeriodReducer::new(
            Frequency::Hourly,
            PeriodCalendar,
            vec![rule(Aggregation::Sum)],
        )
        .expect("reducer builds");
        let mut rows = Vec::new();
        for i in 0..4 {
            reducer.push_row(
                base + i64::from(i) * STEP_MS,
                &[1.0],
                &[true],
                &mut sink(&mut rows),
            );
        }
        collect(reducer, &mut rows);
        assert_eq!(rows.len(), 1, "all four steps sit inside the first hour");
        assert_eq!(
            time_label(rows[0].start_local_ms, site),
            "2023-01-01T00:00:00+05:30"
        );

        let mut reducer = PeriodReducer::new(
            Frequency::Daily,
            PeriodCalendar,
            vec![rule(Aggregation::Sum)],
        )
        .expect("reducer builds");
        let mut rows = Vec::new();
        let evening = local_ms_at(site, 2023, 1, 1, 23, 30);
        let next_day = local_ms_at(site, 2023, 1, 2, 0, 0);
        for local_ms in [evening, evening + STEP_MS, next_day, next_day + STEP_MS] {
            reducer.push_row(local_ms, &[1.0], &[true], &mut sink(&mut rows));
        }
        collect(reducer, &mut rows);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].step_count, 2);
        assert_eq!(rows[1].step_count, 2);
        assert_eq!(
            time_label(rows[0].start_local_ms, site),
            "2023-01-01T00:00:00+05:30"
        );
        assert_eq!(
            time_label(rows[1].start_local_ms, site),
            "2023-01-02T00:00:00+05:30"
        );
    }

    /// A synthetic batch: the Time column, then one Float64 column per
    /// entry, nullable.
    fn batch_with_columns(times: &[&str], columns: &[Vec<Option<f64>>]) -> RecordBatch {
        let mut fields = vec![Field::new("Time", DataType::Utf8, false)];
        let mut arrays: Vec<Arc<dyn Array>> = vec![Arc::new(StringArray::from(times.to_vec()))];
        for (i, column) in columns.iter().enumerate() {
            fields.push(Field::new(format!("c{i}"), DataType::Float64, true));
            arrays.push(Arc::new(Float64Array::from(column.clone())));
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays).expect("batch builds")
    }
}
