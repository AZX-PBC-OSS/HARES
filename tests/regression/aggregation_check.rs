//! Fleet aggregation regression sub-suite.
//!
//! Validates that `hares_fleet::aggregation::aggregate()` produces correct
//! per-dwelling metrics and weighted fleet timeseries from both synthetic data
//! and real Fleet simulation outputs.  The synthetic tests mirror the unit-test
//! logic in `crates/hares-fleet/src/aggregation.rs:491-734` but exercise the
//! public API from the integration-test layer.

use arrow::array::{Array, Float64Array, Float64Builder, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::Duration;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use hares_core::SimulationResults;
use hares_fleet::Fleet;
use hares_fleet::aggregation::{self, AggregationResolution};
use hares_fleet::fleet::{DwellingOutcome, SimStatus};
use hares_io::output::metrics::{
    GridInteractionMetrics, PeakPowerKw, RollingPeakKw, SimulationCoverage, SimulationMetrics,
    TotalEnergyKwh,
};

use super::helpers;

// ---------------------------------------------------------------------------
// Synthetic-data helpers (mirror aggregation.rs unit-test helpers)
// ---------------------------------------------------------------------------

fn sample_metrics(energy: f64, peak: f64) -> SimulationMetrics {
    SimulationMetrics {
        total_energy_kwh: TotalEnergyKwh {
            total: energy,
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
    let mut arrays: Vec<Arc<dyn arrow::array::Array>> =
        vec![Arc::new(StringArray::from(rows.to_vec()))];

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

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

pub fn run_aggregation_check() -> Result<(), Vec<String>> {
    let mut failures = Vec::new();

    run_fleet_integration(&mut failures);
    run_weighted_sum_with_weights(&mut failures);
    run_hourly_resample_suffix_rules(&mut failures);
    run_weighted_mean_vs_sum(&mut failures);
    run_null_propagates_to_aggregate(&mut failures);

    if failures.is_empty() {
        eprintln!("[aggregation_check] PASS");
        Ok(())
    } else {
        Err(failures)
    }
}

// ---------------------------------------------------------------------------
// Integration: fleet simulation → aggregation end-to-end
// ---------------------------------------------------------------------------

fn run_fleet_integration(failures: &mut Vec<String>) {
    let schedule_path = helpers::unique_temp_path("hares-regr-agg-sched", "csv");
    let weather_path = helpers::unique_temp_path("hares-regr-agg-weather", "epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let configs: Vec<_> = (0..3)
        .map(|idx| {
            helpers::build_dwelling_config(
                idx as i64 + 1,
                schedule_path.clone(),
                weather_path.clone(),
                Duration::hours(1),
                idx as u64,
            )
        })
        .collect();

    let fleet = Fleet::from_buildings(configs).with_sample_weights(vec![1.0, 2.0, 0.5]);
    let outcomes = fleet.simulate(1);

    let successful: Vec<DwellingOutcome> = outcomes
        .into_iter()
        .enumerate()
        .filter_map(|(idx, result)| match result {
            Ok(outcome) => {
                if matches!(outcome.status, SimStatus::Failed(_)) {
                    failures.push(format!("dwelling[{idx}] simulation status is Failed"));
                    None
                } else {
                    Some(outcome)
                }
            }
            Err(err) => {
                failures.push(format!("dwelling[{idx}] fleet error: {err}"));
                None
            }
        })
        .collect();

    if successful.len() != 3 {
        failures.push(format!(
            "expected 3 successful dwellings, got {}",
            successful.len()
        ));
    }

    if !successful.is_empty() {
        let results = aggregation::aggregate(&successful, AggregationResolution::Hourly);

        if results.per_dwelling_metrics.len() != successful.len() {
            failures.push(format!(
                "per_dwelling_metrics length {} != dwelling count {}",
                results.per_dwelling_metrics.len(),
                successful.len()
            ));
        }

        for (idx, metrics) in results.per_dwelling_metrics.iter().enumerate() {
            if metrics.total_energy_kwh <= 0.0 || !metrics.total_energy_kwh.is_finite() {
                failures.push(format!(
                    "dwelling[{idx}] total_energy_kwh={} (expected > 0.0)",
                    metrics.total_energy_kwh
                ));
            }
            if metrics.sample_weight <= 0.0 || !metrics.sample_weight.is_finite() {
                failures.push(format!(
                    "dwelling[{idx}] sample_weight={} (expected > 0.0)",
                    metrics.sample_weight
                ));
            }
        }

        if results.aggregate_timeseries.num_rows() == 0 {
            failures.push("aggregate_timeseries is empty".to_string());
        }
    }

    helpers::cleanup_paths(&[schedule_path, weather_path]);
}

// ---------------------------------------------------------------------------
// Synthetic: weighted sum uses sample weights and nulls
// ---------------------------------------------------------------------------

fn run_weighted_sum_with_weights(failures: &mut Vec<String>) {
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

    let fleet = aggregation::aggregate(&[d1, d2, d3, failed], AggregationResolution::FifteenMin);

    if fleet.per_dwelling_metrics.len() != 4 {
        failures.push(format!(
            "per_dwelling_metrics length {} != 4",
            fleet.per_dwelling_metrics.len()
        ));
        return;
    }
    let failed_count = fleet
        .per_dwelling_metrics
        .iter()
        .filter(|m| m.failed)
        .count();
    if failed_count != 1 {
        failures.push(format!("expected 1 failed dwelling, got {failed_count}"));
    }

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

    if times.len() != 2 {
        failures.push(format!("expected 2 time buckets, got {}", times.len()));
        return;
    }

    // 1*1 + 10*2 + 100*0.5 = 71.0
    let v0 = values.value(0);
    if (v0 - 71.0).abs() >= 1e-9 {
        failures.push(format!("weighted sum t0 expected 71.0, got {v0}"));
    }

    // dwelling 2 has null at t1 → aggregate must be null
    if !values.is_null(1) {
        failures.push(format!(
            "weighted sum t1 expected null (dwelling 2 has null), got {:?}",
            if values.is_null(1) {
                "null".to_string()
            } else {
                values.value(1).to_string()
            }
        ));
    }
}

// ---------------------------------------------------------------------------
// Synthetic: hourly resample applies suffix aggregation rules
// ---------------------------------------------------------------------------

fn run_hourly_resample_suffix_rules(failures: &mut Vec<String>) {
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
                    "Temperature - Indoor (\u{b0}C)",
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
                    "Temperature - Indoor (\u{b0}C)",
                    vec![Some(10.0), Some(12.0), Some(14.0), Some(16.0)],
                ),
                (
                    "Battery SOC (-)",
                    vec![Some(0.2), Some(0.4), Some(0.6), Some(0.8)],
                ),
            ],
        )),
    );

    let fleet = aggregation::aggregate(&[d1, d2], AggregationResolution::Hourly);
    if fleet.aggregate_timeseries.num_rows() != 1 {
        failures.push(format!(
            "expected 1 aggregate row, got {}",
            fleet.aggregate_timeseries.num_rows()
        ));
        return;
    }

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
    let energy = col(2);
    let temp = col(3);
    let frac = col(4);

    // Power (kW): Phase-1 mean → Phase-2 weighted sum = (4.0*1) + (5.0*2) = 14.0
    let expected_power = 14.0;
    if (power - expected_power).abs() >= 1e-9 {
        failures.push(format!("power (kW) expected {expected_power}, got {power}"));
    }

    // Energy (kWh): Phase-1 sum → Phase-2 weighted sum = (0.8*1) + (2.0*2) = 4.8
    let expected_energy = 4.8;
    if (energy - expected_energy).abs() >= 1e-9 {
        failures.push(format!(
            "energy (kWh) expected {expected_energy}, got {energy}"
        ));
    }

    // Temperature (°C): weighted mean = (23*1 + 13*2) / (1+2) = 49/3
    let expected_temp = 49.0 / 3.0;
    if (temp - expected_temp).abs() >= 1e-9 {
        failures.push(format!("temp (°C) expected {expected_temp}, got {temp}"));
    }

    // SOC (-): weighted mean = (0.4*1 + 0.5*2) / (1+2) = 1.4/3
    let expected_frac = 1.4 / 3.0;
    if (frac - expected_frac).abs() >= 1e-9 {
        failures.push(format!("SOC (-) expected {expected_frac}, got {frac}"));
    }
}

// ---------------------------------------------------------------------------
// Synthetic: weighted mean vs sum with unequal weights
// ---------------------------------------------------------------------------

fn run_weighted_mean_vs_sum(failures: &mut Vec<String>) {
    let t0 = "2021-01-01T00:00:00Z";

    let make_dwelling = |weight: f64, power: f64, temp: f64, soc: f64| {
        outcome(
            weight,
            SimStatus::Ok,
            sample_metrics(10.0, 1.0),
            Some(batch(
                &[t0],
                vec![
                    ("Total Electric Power (kW)", vec![Some(power)]),
                    ("Temperature - Indoor (\u{b0}C)", vec![Some(temp)]),
                    ("Battery SOC (-)", vec![Some(soc)]),
                ],
            )),
        )
    };

    let d1 = make_dwelling(1.0, 10.0, 20.0, 0.8);
    let d2 = make_dwelling(2.0, 20.0, 30.0, 0.5);
    let d3 = make_dwelling(3.0, 30.0, 25.0, 0.2);

    let fleet = aggregation::aggregate(&[d1, d2, d3], AggregationResolution::Hourly);
    if fleet.aggregate_timeseries.num_rows() != 1 {
        failures.push(format!(
            "weighted mean vs sum: expected 1 row, got {}",
            fleet.aggregate_timeseries.num_rows()
        ));
        return;
    }

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
    let expected_power = 140.0;
    if (power - expected_power).abs() >= 1e-9 {
        failures.push(format!("power (kW) expected {expected_power}, got {power}"));
    }

    // Temperature (°C): weighted mean = (20*1 + 30*2 + 25*3) / (1+2+3) = 155/6
    let expected_temp = (20.0 + 30.0 * 2.0 + 25.0 * 3.0) / 6.0;
    if (temp - expected_temp).abs() >= 1e-9 {
        failures.push(format!("temp (°C) expected {expected_temp}, got {temp}"));
    }

    // SOC (-): weighted mean = (0.8*1 + 0.5*2 + 0.2*3) / 6 = 2.4/6 = 0.4
    let expected_soc = (0.8 + 0.5 * 2.0 + 0.2 * 3.0) / 6.0;
    if (soc - expected_soc).abs() >= 1e-9 {
        failures.push(format!("SOC (-) expected {expected_soc}, got {soc}"));
    }
}

// ---------------------------------------------------------------------------
// Synthetic: null values propagate to aggregate null
// ---------------------------------------------------------------------------

fn run_null_propagates_to_aggregate(failures: &mut Vec<String>) {
    let t0 = "2021-01-01T00:00:00Z";

    let d1 = outcome(
        1.0,
        SimStatus::Ok,
        sample_metrics(10.0, 1.0),
        Some(batch(
            &[t0],
            vec![("Total Electric Power (kW)", vec![Some(1.0)])],
        )),
    );
    let d2 = outcome(
        1.0,
        SimStatus::Ok,
        sample_metrics(10.0, 1.0),
        Some(batch(
            &[t0],
            vec![("Total Electric Power (kW)", vec![None])],
        )),
    );

    let fleet = aggregation::aggregate(&[d1, d2], AggregationResolution::Hourly);
    if fleet.aggregate_timeseries.num_rows() != 1 {
        failures.push(format!(
            "null propagation: expected 1 row, got {}",
            fleet.aggregate_timeseries.num_rows()
        ));
        return;
    }

    let values = fleet
        .aggregate_timeseries
        .column(1)
        .as_any()
        .downcast_ref::<Float64Array>()
        .expect("power column");

    // One dwelling has null at this timestep → aggregate must be null
    if !values.is_null(0) {
        failures.push(format!(
            "null propagation: expected null when one dwelling has null, got {}",
            values.value(0)
        ));
    }
}
