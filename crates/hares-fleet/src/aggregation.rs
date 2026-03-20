//! Weighted output aggregation across dwellings.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use arrow::array::{Array, Float64Array, Float64Builder, StringArray, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, SecondsFormat, Timelike, Utc};

use crate::fleet::{DwellingOutcome, SimStatus};

/// Output timeseries resolution for fleet aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationResolution {
    FifteenMin,
    Hourly,
}

/// Per-dwelling scalar rollup used for fleet reporting.
#[derive(Debug, Clone, PartialEq)]
pub struct DwellingMetrics {
    pub annual_energy_kwh: f64,
    pub peak_power_kw: f64,
    pub sample_weight: f64,
    pub status: SimStatus,
    pub failed: bool,
}

/// Fleet aggregation payload.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetResults {
    pub per_dwelling_metrics: Vec<DwellingMetrics>,
    pub aggregate_timeseries: RecordBatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnAggregation {
    Mean,
    Sum,
}

impl ColumnAggregation {
    fn for_column(name: &str) -> Self {
        let normalized = name.trim();

        if normalized.ends_with("(kWh)") {
            return Self::Sum;
        }

        if normalized.ends_with("(kW)")
            || normalized.ends_with("(C)")
            || normalized.ends_with("(\u{b0}C)")
            || normalized.ends_with("(-)")
        {
            return Self::Mean;
        }

        Self::Mean
    }
}

/// Phase 2 (cross-dwelling fleet) aggregation rule.
///
/// Distinct from `ColumnAggregation` which governs Phase 1 (temporal resampling).
/// Matches OCHRE `agg_by="House"` semantics: temperature and dimensionless
/// ratios use weighted mean; everything else uses weighted sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FleetAggregation {
    WeightedSum,
    WeightedMean,
}

impl FleetAggregation {
    fn for_column(name: &str) -> Self {
        let normalized = name.trim();

        if normalized.ends_with("(C)")
            || normalized.ends_with("(\u{b0}C)")
            || normalized.ends_with("(-)")
        {
            return Self::WeightedMean;
        }

        Self::WeightedSum
    }
}

#[derive(Debug, Clone)]
struct Accumulator {
    sum: f64,
    count: usize,
    saw_null: bool,
}

impl Accumulator {
    fn new() -> Self {
        Self {
            sum: 0.0,
            count: 0,
            saw_null: false,
        }
    }

    fn add(&mut self, value: Option<f64>) {
        match value {
            Some(v) => {
                self.sum += v;
                self.count += 1;
            }
            None => {
                self.saw_null = true;
            }
        }
    }

    fn finish(self, mode: ColumnAggregation) -> Option<f64> {
        if self.saw_null || self.count == 0 {
            return None;
        }

        match mode {
            ColumnAggregation::Sum => Some(self.sum),
            ColumnAggregation::Mean => Some(self.sum / self.count as f64),
        }
    }
}

/// Aggregate dwelling outcomes into per-dwelling metrics and weighted fleet timeseries.
#[must_use]
pub fn aggregate(results: &[DwellingOutcome], resolution: AggregationResolution) -> FleetResults {
    let per_dwelling_metrics = results
        .iter()
        .map(|outcome| DwellingMetrics {
            annual_energy_kwh: outcome.result.metrics.annual_energy_kwh.total,
            peak_power_kw: outcome
                .result
                .metrics
                .grid_interaction_metrics
                .peak_import_kw,
            sample_weight: outcome.sample_weight,
            status: outcome.status.clone(),
            failed: matches!(outcome.status, SimStatus::Failed(_)),
        })
        .collect();

    let mut successful = Vec::new();
    for outcome in results {
        if matches!(outcome.status, SimStatus::Failed(_)) {
            continue;
        }

        let Some((schema, rows)) = extract_rows(outcome) else {
            continue;
        };
        if rows.is_empty() {
            continue;
        }

        let numeric_columns = numeric_column_indexes(&schema);
        if numeric_columns.is_empty() {
            continue;
        }

        let column_names: Vec<String> = numeric_columns
            .iter()
            .map(|idx| schema.field(*idx).name().clone())
            .collect();
        let aggregators: Vec<ColumnAggregation> = column_names
            .iter()
            .map(|name| ColumnAggregation::for_column(name))
            .collect();

        let buckets = resample_rows(&rows, &numeric_columns, &aggregators, resolution);
        successful.push((outcome.sample_weight, column_names, buckets));
    }

    let aggregate_timeseries = build_aggregate_batch(successful);

    FleetResults {
        per_dwelling_metrics,
        aggregate_timeseries,
    }
}

fn extract_rows(outcome: &DwellingOutcome) -> Option<(Arc<Schema>, Vec<RecordBatch>)> {
    let batches = outcome.result.timeseries.as_ref()?;
    let first = batches.first()?;
    let schema = first.schema();

    if !batches
        .iter()
        .all(|batch| batch.schema().fields() == schema.fields())
    {
        return None;
    }

    Some((schema, batches.clone()))
}

fn numeric_column_indexes(schema: &Schema) -> Vec<usize> {
    schema
        .fields()
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(idx, field)| {
            if matches!(field.data_type(), DataType::Float64) {
                Some(idx)
            } else {
                None
            }
        })
        .collect()
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|ts| ts.with_timezone(&Utc))
}

fn bucket_start(ts: DateTime<Utc>, resolution: AggregationResolution) -> DateTime<Utc> {
    match resolution {
        AggregationResolution::Hourly => ts
            .with_minute(0)
            .and_then(|x| x.with_second(0))
            .and_then(|x| x.with_nanosecond(0))
            .unwrap_or(ts),
        AggregationResolution::FifteenMin => {
            let minute = (ts.minute() / 15) * 15;
            ts.with_minute(minute)
                .and_then(|x| x.with_second(0))
                .and_then(|x| x.with_nanosecond(0))
                .unwrap_or(ts)
        }
    }
}

fn value_from_column(batch: &RecordBatch, column_idx: usize, row_idx: usize) -> Option<f64> {
    let column = batch.column(column_idx);
    if column.is_null(row_idx) {
        return None;
    }

    match column.data_type() {
        DataType::Float64 => column
            .as_any()
            .downcast_ref::<Float64Array>()
            .map(|arr| arr.value(row_idx)),
        _ => None,
    }
}

fn resample_rows(
    rows: &[RecordBatch],
    numeric_columns: &[usize],
    aggregators: &[ColumnAggregation],
    resolution: AggregationResolution,
) -> BTreeMap<i64, Vec<Option<f64>>> {
    let mut per_bucket: HashMap<i64, Vec<Accumulator>> = HashMap::new();

    for batch in rows {
        if batch.num_columns() == 0 {
            continue;
        }

        let Some(time_col) = batch.column(0).as_any().downcast_ref::<StringArray>() else {
            continue;
        };

        for row_idx in 0..batch.num_rows() {
            if time_col.is_null(row_idx) {
                continue;
            }

            let Some(ts) = parse_timestamp(time_col.value(row_idx)) else {
                continue;
            };
            let bucket = bucket_start(ts, resolution).timestamp();

            let accs = per_bucket.entry(bucket).or_insert_with(|| {
                (0..numeric_columns.len())
                    .map(|_| Accumulator::new())
                    .collect()
            });

            for (slot, col_idx) in numeric_columns.iter().enumerate() {
                let value = value_from_column(batch, *col_idx, row_idx);
                accs[slot].add(value);
            }
        }
    }

    let mut out = BTreeMap::new();
    for (bucket, mut accs) in per_bucket {
        let values = accs
            .drain(..)
            .zip(aggregators.iter().copied())
            .map(|(acc, mode)| acc.finish(mode))
            .collect();
        out.insert(bucket, values);
    }

    out
}

type AggregateEntry = (f64, Vec<String>, BTreeMap<i64, Vec<Option<f64>>>);

fn build_aggregate_batch(successful: Vec<AggregateEntry>) -> RecordBatch {
    let Some((_, first_columns, first_buckets)) = successful.first() else {
        return empty_batch();
    };

    let column_count = first_columns.len();
    let fleet_aggs: Vec<FleetAggregation> = first_columns
        .iter()
        .map(|name| FleetAggregation::for_column(name))
        .collect();

    let mut bucket_intersection: BTreeSet<i64> = first_buckets.keys().copied().collect();
    for (_, columns, buckets) in successful.iter().skip(1) {
        if columns != first_columns {
            return empty_batch();
        }
        let keys: BTreeSet<i64> = buckets.keys().copied().collect();
        bucket_intersection = bucket_intersection
            .intersection(&keys)
            .copied()
            .collect::<BTreeSet<_>>();
    }

    let mut timestamps = StringBuilder::new();
    let mut numeric_builders: Vec<Float64Builder> =
        (0..column_count).map(|_| Float64Builder::new()).collect();

    for bucket in bucket_intersection {
        if let Some(dt) = DateTime::<Utc>::from_timestamp(bucket, 0) {
            timestamps.append_value(dt.to_rfc3339_opts(SecondsFormat::Secs, true));
        } else {
            continue;
        }

        let mut weighted_values = vec![0.0; column_count];
        let mut total_weight = vec![0.0; column_count];
        let mut has_null = vec![false; column_count];

        for (sample_weight, _cols, buckets) in &successful {
            let Some(values) = buckets.get(&bucket) else {
                has_null.fill(true);
                continue;
            };

            for (idx, value) in values.iter().enumerate() {
                match value {
                    Some(v) => {
                        weighted_values[idx] += *v * *sample_weight;
                        total_weight[idx] += *sample_weight;
                    }
                    None => has_null[idx] = true,
                }
            }
        }

        for (idx, builder) in numeric_builders.iter_mut().enumerate() {
            if has_null[idx] {
                builder.append_null();
            } else {
                let value = match fleet_aggs[idx] {
                    FleetAggregation::WeightedSum => weighted_values[idx],
                    FleetAggregation::WeightedMean if total_weight[idx] > 0.0 => {
                        weighted_values[idx] / total_weight[idx]
                    }
                    FleetAggregation::WeightedMean => {
                        builder.append_null();
                        continue;
                    }
                };
                builder.append_value(value);
            }
        }
    }

    let mut fields = Vec::with_capacity(column_count + 1);
    fields.push(Field::new("Time", DataType::Utf8, false));
    for col in first_columns {
        fields.push(Field::new(col, DataType::Float64, true));
    }

    let schema = Arc::new(Schema::new(fields));
    let mut arrays: Vec<Arc<dyn Array>> = Vec::with_capacity(column_count + 1);
    arrays.push(Arc::new(timestamps.finish()));
    for mut builder in numeric_builders {
        arrays.push(Arc::new(builder.finish()));
    }

    RecordBatch::try_new(schema, arrays).unwrap_or_else(|_| empty_batch())
}

fn empty_batch() -> RecordBatch {
    let schema = Arc::new(Schema::new(vec![Field::new("Time", DataType::Utf8, false)]));
    let arrays: Vec<Arc<dyn Array>> = vec![Arc::new(StringArray::from(Vec::<&str>::new()))];
    RecordBatch::try_new(schema, arrays).expect("empty aggregate record batch")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration as StdDuration;

    use arrow::datatypes::Schema;
    use hares_core::SimulationResults;
    use hares_io::output::metrics::{
        AnnualEnergyKwh, GridInteractionMetrics, PeakPowerKw, RollingPeakKw, SimulationMetrics,
    };

    fn sample_metrics(energy: f64, peak: f64) -> SimulationMetrics {
        SimulationMetrics {
            annual_energy_kwh: AnnualEnergyKwh {
                total: energy,
                per_end_use: BTreeMap::new(),
            },
            peak_power_kw: PeakPowerKw {
                per_end_use: BTreeMap::new(),
                rolling: RollingPeakKw {
                    peak_15min_kw: 0.0,
                    peak_30min_kw: 0.0,
                    peak_60min_kw: 0.0,
                },
            },
            comfort_hours: None,
            unmet_load_hours: None,
            renewable_energy_fraction: None,
            grid_interaction_metrics: GridInteractionMetrics {
                peak_import_kw: peak,
                peak_export_kw: 0.0,
            },
        }
    }

    fn outcome(
        sample_weight: f64,
        status: SimStatus,
        metrics: SimulationMetrics,
        batch: Option<RecordBatch>,
    ) -> DwellingOutcome {
        DwellingOutcome {
            result: SimulationResults {
                timeseries_path: None,
                timeseries: batch.map(|b| vec![b]),
                metrics,
                warnings: Vec::new(),
                status: hares_core::SimStatus::Ok,
                elapsed: StdDuration::from_secs(1),
            },
            sample_weight,
            status,
        }
    }

    fn batch(rows: &[&str], columns: Vec<(&str, Vec<Option<f64>>)>) -> RecordBatch {
        let mut fields = vec![Field::new("Time", DataType::Utf8, false)];
        let mut arrays: Vec<Arc<dyn Array>> = vec![Arc::new(StringArray::from(rows.to_vec()))];

        for (name, values) in columns {
            fields.push(Field::new(name, DataType::Float64, true));
            let mut builder = Float64Builder::new();
            for value in values {
                match value {
                    Some(v) => builder.append_value(v),
                    None => builder.append_null(),
                }
            }
            arrays.push(Arc::new(builder.finish()));
        }

        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, arrays).expect("valid test batch")
    }

    #[test]
    fn weighted_sum_uses_sample_weights_and_nulls() {
        let t0 = "2021-01-01T00:00:00Z";
        let t1 = "2021-01-01T00:15:00Z";
        let t2 = "2021-01-01T00:30:00Z";

        let d1 = outcome(
            1.0,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0, t1, t2],
                vec![(
                    "Total Electric Power (kW)",
                    vec![Some(1.0), Some(2.0), Some(3.0)],
                )],
            )),
        );
        let d2 = outcome(
            2.0,
            SimStatus::Ok,
            sample_metrics(11.0, 2.0),
            Some(batch(
                &[t0, t1, t2],
                vec![(
                    "Total Electric Power (kW)",
                    vec![Some(10.0), None, Some(30.0)],
                )],
            )),
        );
        let d3 = outcome(
            0.5,
            SimStatus::Ok,
            sample_metrics(12.0, 3.0),
            Some(batch(
                &[t0, t1],
                vec![("Total Electric Power (kW)", vec![Some(100.0), Some(200.0)])],
            )),
        );
        let failed = outcome(
            5.0,
            SimStatus::Failed("boom".to_string()),
            sample_metrics(0.0, 0.0),
            Some(batch(
                &[t0, t1, t2],
                vec![(
                    "Total Electric Power (kW)",
                    vec![Some(999.0), Some(999.0), Some(999.0)],
                )],
            )),
        );

        let fleet = aggregate(&[d1, d2, d3, failed], AggregationResolution::FifteenMin);
        assert_eq!(fleet.per_dwelling_metrics.len(), 4);
        assert_eq!(
            fleet
                .per_dwelling_metrics
                .iter()
                .filter(|m| m.failed)
                .count(),
            1
        );

        let times = fleet
            .aggregate_timeseries
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("time column");
        let values = fleet
            .aggregate_timeseries
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("power column");

        // Intersection removes 00:30 because dwelling 3 ended early.
        assert_eq!(times.len(), 2);
        assert_eq!(times.value(0), t0);
        assert_eq!(times.value(1), t1);

        // 1*1 + 10*2 + 100*0.5 = 71.0
        assert!((values.value(0) - 71.0).abs() < 1e-9);
        // dwelling 2 has null at t1, so aggregate must be null (not zero)
        assert!(values.is_null(1));
    }

    #[test]
    fn hourly_resample_applies_suffix_aggregation_rules() {
        let rows = [
            "2021-01-01T00:00:00Z",
            "2021-01-01T00:15:00Z",
            "2021-01-01T00:30:00Z",
            "2021-01-01T00:45:00Z",
        ];

        let d1 = outcome(
            1.0,
            SimStatus::Ok,
            sample_metrics(20.0, 2.0),
            Some(batch(
                &rows,
                vec![
                    (
                        "Total Electric Power (kW)",
                        vec![Some(1.0), Some(3.0), Some(5.0), Some(7.0)],
                    ),
                    (
                        "Battery Energy (kWh)",
                        vec![Some(0.2), Some(0.2), Some(0.2), Some(0.2)],
                    ),
                    (
                        "Temperature - Indoor (C)",
                        vec![Some(20.0), Some(22.0), Some(24.0), Some(26.0)],
                    ),
                    (
                        "Battery SOC (-)",
                        vec![Some(0.1), Some(0.3), Some(0.5), Some(0.7)],
                    ),
                ],
            )),
        );

        let d2 = outcome(
            2.0,
            SimStatus::Ok,
            sample_metrics(30.0, 3.0),
            Some(batch(
                &rows,
                vec![
                    (
                        "Total Electric Power (kW)",
                        vec![Some(2.0), Some(4.0), Some(6.0), Some(8.0)],
                    ),
                    (
                        "Battery Energy (kWh)",
                        vec![Some(0.5), Some(0.5), Some(0.5), Some(0.5)],
                    ),
                    (
                        "Temperature - Indoor (C)",
                        vec![Some(10.0), Some(12.0), Some(14.0), Some(16.0)],
                    ),
                    (
                        "Battery SOC (-)",
                        vec![Some(0.2), Some(0.4), Some(0.6), Some(0.8)],
                    ),
                ],
            )),
        );

        let fleet = aggregate(&[d1, d2], AggregationResolution::Hourly);
        assert_eq!(fleet.aggregate_timeseries.num_rows(), 1);

        let power = fleet
            .aggregate_timeseries
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("power column")
            .value(0);
        let energy = fleet
            .aggregate_timeseries
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("energy column")
            .value(0);
        let temp = fleet
            .aggregate_timeseries
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("temp column")
            .value(0);
        let frac = fleet
            .aggregate_timeseries
            .column(4)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("fraction column")
            .value(0);

        // Power uses mean then weighted sum: (4.0*1) + (5.0*2) = 14.0
        assert!((power - 14.0).abs() < 1e-9);
        // Energy uses sum then weighted sum: (0.8*1) + (2.0*2) = 4.8
        assert!((energy - 4.8).abs() < 1e-9);
        // Temperature uses weighted mean: (23*1 + 13*2) / (1+2) = 49/3
        assert!((temp - 49.0 / 3.0).abs() < 1e-9);
        // Fraction uses weighted mean: (0.4*1 + 0.5*2) / (1+2) = 1.4/3
        assert!((frac - 1.4 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn fleet_weighted_mean_vs_sum_with_unequal_weights() {
        let t0 = "2021-01-01T00:00:00Z";

        // Three dwellings with weights 1, 2, 3
        let make_dwelling = |weight: f64, power: f64, temp: f64, soc: f64| {
            outcome(
                weight,
                SimStatus::Ok,
                sample_metrics(10.0, 1.0),
                Some(batch(
                    &[t0],
                    vec![
                        ("Total Electric Power (kW)", vec![Some(power)]),
                        ("Temperature - Indoor (C)", vec![Some(temp)]),
                        ("Battery SOC (-)", vec![Some(soc)]),
                    ],
                )),
            )
        };

        let d1 = make_dwelling(1.0, 10.0, 20.0, 0.8);
        let d2 = make_dwelling(2.0, 20.0, 30.0, 0.5);
        let d3 = make_dwelling(3.0, 30.0, 25.0, 0.2);

        let fleet = aggregate(&[d1, d2, d3], AggregationResolution::Hourly);
        assert_eq!(fleet.aggregate_timeseries.num_rows(), 1);

        let col = |idx: usize| -> f64 {
            fleet
                .aggregate_timeseries
                .column(idx)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(0)
        };

        let power = col(1);
        let temp = col(2);
        let soc = col(3);

        // Power (kW): weighted sum = 10*1 + 20*2 + 30*3 = 140
        assert!((power - 140.0).abs() < 1e-9);

        // Temperature (C): weighted mean = (20*1 + 30*2 + 25*3) / (1+2+3) = 155/6
        let expected_temp = (20.0 + 30.0 * 2.0 + 25.0 * 3.0) / 6.0;
        assert!((temp - expected_temp).abs() < 1e-9);

        // SOC (-): weighted mean = (0.8*1 + 0.5*2 + 0.2*3) / (1+2+3) = 2.4/6 = 0.4
        let expected_soc = (0.8 + 0.5 * 2.0 + 0.2 * 3.0) / 6.0;
        assert!((soc - expected_soc).abs() < 1e-9);
    }
}
