//! Weighted output aggregation across dwellings.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use arrow::array::{Array, Float64Array, Float64Builder, StringArray, StringBuilder};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, FixedOffset, SecondsFormat, Timelike};

use crate::fleet::{DwellingOutcome, FleetError, SimStatus};

/// Output timeseries resolution for fleet aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationResolution {
    FifteenMin,
    Hourly,
}

/// Per-dwelling scalar rollup used for fleet reporting.
#[derive(Debug, Clone, PartialEq)]
pub struct DwellingMetrics {
    pub total_energy_kwh: f64,
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
pub fn aggregate(
    results: &[DwellingOutcome],
    resolution: AggregationResolution,
) -> Result<FleetResults, FleetError> {
    let per_dwelling_metrics = results
        .iter()
        .map(|outcome| DwellingMetrics {
            total_energy_kwh: outcome.result.metrics.total_energy_kwh.net_energy_kwh,
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
    for (index, outcome) in results.iter().enumerate() {
        if matches!(outcome.status, SimStatus::Failed(_)) {
            continue;
        }

        // Defense-in-depth: a single non-finite or negative weight corrupts
        // `weighted_values`/`total_weight` for every column it touches,
        // regardless of whether other dwellings have valid weights. Reject it
        // here even though the primary ingestion boundaries (ResStock parquet
        // parsing and `Fleet::with_sample_weights`) already validate, so any
        // future path that constructs a `DwellingOutcome` directly cannot
        // silently poison the aggregate.
        if hares_io::classify_sample_weight(outcome.sample_weight)
            == hares_io::SampleWeightClass::Invalid
        {
            return Err(FleetError::InvalidAggregationWeight {
                index,
                value: outcome.sample_weight,
            });
        }

        let Some((schema, rows)) = extract_rows(outcome, index) else {
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

    // Every weight in `successful` is now finite and non-negative, so the only
    // remaining degenerate case is a fleet where all contributing weights are
    // zero — the population estimate would be undefined.
    let has_positive_weight = successful.iter().any(|(w, _, _)| *w > 0.0);
    if !successful.is_empty() && !has_positive_weight {
        return Err(FleetError::ZeroWeightFleet);
    }

    #[cfg(feature = "observe")]
    {
        let n_successful = successful.len();
        // Weights are guaranteed finite and non-negative by the validation
        // above, so filtering on `> 0.0` is sufficient to isolate contributors.
        let positive_weights: Vec<f64> = successful
            .iter()
            .map(|(w, _, _)| *w)
            .filter(|w| *w > 0.0)
            .collect();
        if !positive_weights.is_empty() {
            let min = positive_weights
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let max = positive_weights
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            let mean = positive_weights.iter().sum::<f64>() / positive_weights.len() as f64;
            tracing::info!(
                target: "observe",
                n_dwellings = n_successful,
                weight_min = min,
                weight_max = max,
                weight_mean = mean,
                "fleet aggregation weight distribution",
            );
        }
    }

    let n_successful = successful.len();
    let aggregate_timeseries = build_aggregate_batch(successful);

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        if aggregate_timeseries.num_rows() == 0 && n_successful > 0 {
            tracing::error!(
                n_successful = n_successful,
                "fleet aggregation invariant violated: aggregate timeseries is empty \
                 despite successful dwelling(s)",
            );
        }
    }

    Ok(FleetResults {
        per_dwelling_metrics,
        aggregate_timeseries,
    })
}

fn extract_rows(
    outcome: &DwellingOutcome,
    dwelling_index: usize,
) -> Option<(Arc<Schema>, Vec<RecordBatch>)> {
    let batches = outcome.result.timeseries.as_ref()?;
    let first = batches.first()?;
    let schema = first.schema();

    for (batch_idx, batch) in batches.iter().enumerate().skip(1) {
        let batch_schema = batch.schema();
        let batch_fields = batch_schema.fields();
        if batch_fields != schema.fields() {
            let expected_names: Vec<&str> =
                schema.fields().iter().map(|f| f.name().as_str()).collect();
            let actual_names: Vec<&str> = batch_fields.iter().map(|f| f.name().as_str()).collect();
            let expected_set: BTreeSet<_> = expected_names.iter().copied().collect();
            let actual_set: BTreeSet<_> = actual_names.iter().copied().collect();
            let missing: Vec<_> = expected_set.difference(&actual_set).copied().collect();
            let extra: Vec<_> = actual_set.difference(&expected_set).copied().collect();

            tracing::warn!(
                dwelling_index = dwelling_index,
                batch_index = batch_idx,
                expected_fields = ?expected_names,
                mismatched_fields = ?actual_names,
                missing = ?missing,
                extra = ?extra,
                "intra-dwelling schema mismatch in extract_rows; excluding dwelling from aggregation",
            );

            #[cfg(feature = "observe")]
            {
                tracing::info!(
                    target: "observe",
                    dwelling_index = dwelling_index,
                    reason = "intra_dwelling_schema_mismatch",
                    "dwelling excluded from fleet aggregation due to intra-dwelling schema mismatch",
                );
            }

            return None;
        }
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

fn parse_timestamp(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

fn bucket_start(
    ts: DateTime<FixedOffset>,
    resolution: AggregationResolution,
) -> DateTime<FixedOffset> {
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

    #[cfg(feature = "observe")]
    {
        let all_schemas_match = successful
            .iter()
            .skip(1)
            .all(|(_, cols, _)| cols == first_columns);
        tracing::info!(
            target: "observe",
            n_columns = first_columns.len(),
            all_schemas_match = all_schemas_match,
            "fleet aggregate schema consistency",
        );
    }

    let mut bucket_intersection: BTreeSet<i64> = first_buckets.keys().copied().collect();
    for (_, columns, buckets) in successful.iter().skip(1) {
        if columns != first_columns {
            let first_set: BTreeSet<_> = first_columns.iter().collect();
            let other_set: BTreeSet<_> = columns.iter().collect();
            let only_in_first: Vec<_> = first_set.difference(&other_set).collect();
            let only_in_other: Vec<_> = other_set.difference(&first_set).collect();
            tracing::warn!(
                n_first = first_columns.len(),
                n_other = columns.len(),
                only_in_first = ?only_in_first,
                only_in_other = ?only_in_other,
                "column mismatch in fleet aggregation; returning empty batch",
            );
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
        if let Some(dt) = DateTime::from_timestamp(bucket, 0).map(|dt| dt.fixed_offset()) {
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

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            for (idx, &tw) in total_weight.iter().enumerate() {
                if !tw.is_finite() || tw < 0.0 {
                    tracing::error!(
                        column = idx,
                        total_weight = tw,
                        bucket = bucket,
                        "fleet aggregation invariant violated: total_weight is non-finite or negative",
                    );
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

    RecordBatch::try_new(schema, arrays).unwrap_or_else(|e| {
        tracing::error!(
            error = %e,
            "fleet aggregation: RecordBatch::try_new failed, returning empty batch",
        );
        empty_batch()
    })
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
        GridInteractionMetrics, PeakPowerKw, Reliability, RollingPeakKw, SimulationCoverage,
        SimulationMetrics, TotalEnergyKwh,
    };

    fn sample_metrics(energy: f64, peak: f64) -> SimulationMetrics {
        SimulationMetrics {
            total_energy_kwh: TotalEnergyKwh {
                net_energy_kwh: energy,
                gross_consumption_kwh: energy,
                gross_pv_generation_kwh: 0.0,
                per_end_use: BTreeMap::new(),
                duration_hours: 0.0,
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
            envelope_loads_kwh: None,
            efficiency: Default::default(),
            rows_with_partial_setpoint_data_fraction: None,
            simulation_duration_hours: 0.0,
            coverage: SimulationCoverage::PartialYear,
            nan_step_count: 0,
            metrics_reliability: Reliability::Reliable,
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

        let fleet =
            aggregate(&[d1, d2, d3, failed], AggregationResolution::FifteenMin).expect("aggregate");
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

        let fleet = aggregate(&[d1, d2], AggregationResolution::Hourly).expect("aggregate");
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

        let fleet = aggregate(&[d1, d2, d3], AggregationResolution::Hourly).expect("aggregate");
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

    #[test]
    fn aggregate_empty_dwellings_returns_empty_results() {
        let fleet = aggregate(&[], AggregationResolution::FifteenMin).expect("aggregate");
        assert!(fleet.per_dwelling_metrics.is_empty());
        assert_eq!(fleet.aggregate_timeseries.num_rows(), 0);

        let fleet_hourly = aggregate(&[], AggregationResolution::Hourly).expect("aggregate");
        assert!(fleet_hourly.per_dwelling_metrics.is_empty());
        assert_eq!(fleet_hourly.aggregate_timeseries.num_rows(), 0);
    }

    #[test]
    fn aggregate_identical_dwellings_preserves_weighted_mean_identity_and_sums_correctly() {
        let t0 = "2021-01-01T00:00:00Z";
        let t1 = "2021-01-01T00:15:00Z";

        // Two dwellings with identical timeseries but different weights.
        // Weighted-sum columns (kW, kWh): fleet = value * (w1 + w2)
        // Weighted-mean columns (C, -): fleet = (value*w1 + value*w2) / (w1+w2) = value
        let make_dwelling = |weight: f64| -> DwellingOutcome {
            outcome(
                weight,
                SimStatus::Ok,
                sample_metrics(10.0, 1.0),
                Some(batch(
                    &[t0, t1],
                    vec![
                        ("Total Electric Power (kW)", vec![Some(1.5), Some(3.0)]),
                        ("Battery Energy (kWh)", vec![Some(0.2), Some(0.4)]),
                        ("Temperature - Indoor (C)", vec![Some(21.0), Some(22.0)]),
                        ("Battery SOC (-)", vec![Some(0.7), Some(0.8)]),
                    ],
                )),
            )
        };

        let d1 = make_dwelling(1.0);
        let d2 = make_dwelling(2.0);

        let fleet = aggregate(&[d1, d2], AggregationResolution::FifteenMin).expect("aggregate");
        assert_eq!(fleet.per_dwelling_metrics.len(), 2);
        assert_eq!(fleet.aggregate_timeseries.num_rows(), 2);

        let col = |idx: usize, row: usize| -> f64 {
            fleet
                .aggregate_timeseries
                .column(idx)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(row)
        };

        // Power (kW): weighted sum = 1.5*1 + 1.5*2 = 4.5 at t0, 3.0*1 + 3.0*2 = 9.0 at t1
        assert!((col(1, 0) - 4.5).abs() < 1e-9);
        assert!((col(1, 1) - 9.0).abs() < 1e-9);

        // Energy (kWh): weighted sum = 0.2*1 + 0.2*2 = 0.6, 0.4*1 + 0.4*2 = 1.2
        assert!((col(2, 0) - 0.6).abs() < 1e-9);
        assert!((col(2, 1) - 1.2).abs() < 1e-9);

        // Temperature (C): weighted mean = (21*1 + 21*2)/(1+2) = 21.0, (22*1 + 22*2)/(1+2) = 22.0
        assert!((col(3, 0) - 21.0).abs() < 1e-9);
        assert!((col(3, 1) - 22.0).abs() < 1e-9);

        // SOC (-): weighted mean = (0.7*1 + 0.7*2)/(1+2) = 0.7, (0.8*1 + 0.8*2)/(1+2) = 0.8
        assert!((col(4, 0) - 0.7).abs() < 1e-9);
        assert!((col(4, 1) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn aggregation_with_zero_weights() {
        let t0 = "2021-01-01T00:00:00Z";

        let zero_dwelling = outcome(
            0.0,
            SimStatus::Ok,
            sample_metrics(0.0, 0.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(100.0)])],
            )),
        );
        let normal_dwelling = outcome(
            2.0,
            SimStatus::Ok,
            sample_metrics(10.0, 2.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(10.0)])],
            )),
        );

        let fleet = aggregate(
            &[zero_dwelling, normal_dwelling],
            AggregationResolution::Hourly,
        )
        .expect("aggregate with mixed zero and positive weights");

        let values = fleet
            .aggregate_timeseries
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("power column");
        // Zero-weight dwelling contributes 0, so power = 10.0 * 2.0 = 20.0
        assert!((values.value(0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_rejects_single_invalid_weight_among_valid() {
        let t0 = "2021-01-01T00:00:00Z";

        // One valid dwelling and one NaN-weighted dwelling. The all-invalid
        // guard would pass this (a positive weight exists), so aggregation must
        // reject the individual NaN before it poisons the weighted sums.
        let valid = outcome(
            1.0,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(10.0)])],
            )),
        );
        let poisoned = outcome(
            f64::NAN,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(20.0)])],
            )),
        );

        let err = aggregate(&[valid, poisoned], AggregationResolution::Hourly).unwrap_err();
        match err {
            FleetError::InvalidAggregationWeight { index, value } => {
                assert_eq!(index, 1);
                assert!(value.is_nan());
            }
            other => panic!("expected InvalidAggregationWeight, got {other:?}"),
        }
    }

    #[test]
    fn aggregate_rejects_negative_weight() {
        let t0 = "2021-01-01T00:00:00Z";

        let valid = outcome(
            1.0,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(10.0)])],
            )),
        );
        let negative = outcome(
            -2.0,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(20.0)])],
            )),
        );

        let err = aggregate(&[valid, negative], AggregationResolution::Hourly).unwrap_err();
        match err {
            FleetError::InvalidAggregationWeight { index, value } => {
                assert_eq!(index, 1);
                assert_eq!(value, -2.0);
            }
            other => panic!("expected InvalidAggregationWeight, got {other:?}"),
        }
    }

    #[test]
    fn aggregate_invalid_weight_index_counts_all_input_outcomes() {
        let t0 = "2021-01-01T00:00:00Z";

        // A failed dwelling precedes the invalid-weight dwelling. The reported
        // index counts every input outcome (not just successful ones), so the
        // caller can locate the offending dwelling in the original slice.
        let failed = outcome(
            1.0,
            SimStatus::Failed("boom".to_string()),
            sample_metrics(0.0, 0.0),
            None,
        );
        let poisoned = outcome(
            f64::INFINITY,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(20.0)])],
            )),
        );

        let err = aggregate(&[failed, poisoned], AggregationResolution::Hourly).unwrap_err();
        match err {
            FleetError::InvalidAggregationWeight { index, value } => {
                assert_eq!(index, 1);
                assert!(value.is_infinite());
            }
            other => panic!("expected InvalidAggregationWeight, got {other:?}"),
        }
    }

    #[test]
    fn fleet_rejects_all_zero_weights() {
        let t0 = "2021-01-01T00:00:00Z";

        let d1 = outcome(
            0.0,
            SimStatus::Ok,
            sample_metrics(0.0, 0.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(1.0)])],
            )),
        );
        let d2 = outcome(
            0.0,
            SimStatus::Ok,
            sample_metrics(0.0, 0.0),
            Some(batch(
                &[t0],
                vec![("Total Electric Power (kW)", vec![Some(2.0)])],
            )),
        );

        let err = aggregate(&[d1, d2], AggregationResolution::Hourly).unwrap_err();
        assert!(matches!(err, FleetError::ZeroWeightFleet));
    }

    #[test]
    fn column_mismatch_returns_empty_batch() {
        let t0 = "2021-01-01T00:00:00Z";

        let d1 = outcome(
            1.0,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![
                    ("Total Electric Power (kW)", vec![Some(10.0)]),
                    ("Temperature - Indoor (C)", vec![Some(21.0)]),
                ],
            )),
        );
        let d2 = outcome(
            1.0,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![
                    ("Total Electric Power (kW)", vec![Some(20.0)]),
                    ("Battery SOC (-)", vec![Some(0.5)]),
                ],
            )),
        );

        let fleet = aggregate(&[d1, d2], AggregationResolution::FifteenMin).expect("aggregate");
        assert_eq!(
            fleet.aggregate_timeseries.num_rows(),
            0,
            "column mismatch should produce empty batch"
        );
        assert_eq!(fleet.aggregate_timeseries.num_columns(), 1);
        assert_eq!(fleet.per_dwelling_metrics.len(), 2);
    }

    #[test]
    fn extract_rows_returns_none_on_intra_dwelling_schema_mismatch() {
        let batch1 = batch(
            &["2021-01-01T00:00:00Z"],
            vec![("Total Electric Power (kW)", vec![Some(1.0)])],
        );
        let batch2 = batch(
            &["2021-01-01T00:15:00Z"],
            vec![("Temperature - Indoor (C)", vec![Some(21.0)])],
        );

        let outcome = DwellingOutcome {
            result: SimulationResults {
                timeseries_path: None,
                timeseries: Some(vec![batch1, batch2]),
                metrics: sample_metrics(10.0, 1.0),
                warnings: Vec::new(),
                status: hares_core::SimStatus::Ok,
                elapsed: StdDuration::from_secs(1),
            },
            sample_weight: 1.0,
            status: SimStatus::Ok,
        };

        let result = extract_rows(&outcome, 0);
        assert!(result.is_none());
    }
}
