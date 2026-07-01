mod common;

use std::collections::BTreeMap;
use std::time::{Duration as StdDuration, Instant};

use arrow::array::{Float64Builder, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, FixedOffset, TimeZone};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hares_core::{SimStatus as CoreSimStatus, SimulationResults};
use hares_fleet::aggregation::aggregate;
use hares_fleet::{AggregationResolution, DwellingOutcome, SimStatus};
use hares_io::output::metrics::{
    GridInteractionMetrics, PeakPowerKw, Reliability, RollingPeakKw, SimulationCoverage,
    SimulationMetrics, TotalEnergyKwh,
};
use std::sync::Arc;

fn empty_metrics() -> SimulationMetrics {
    SimulationMetrics {
        total_energy_kwh: TotalEnergyKwh {
            net_energy_kwh: 0.0,
            gross_consumption_kwh: 0.0,
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
            peak_import_kw: 0.0,
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

fn outcome_from_batch(batch: RecordBatch, sample_weight: f64) -> DwellingOutcome {
    DwellingOutcome {
        result: SimulationResults {
            timeseries_path: None,
            timeseries: Some(vec![batch]),
            metrics: empty_metrics(),
            warnings: Vec::new(),
            status: CoreSimStatus::Ok,
            elapsed: StdDuration::from_secs(1),
        },
        sample_weight,
        status: SimStatus::Ok,
    }
}

const N_COLUMNS: usize = 4;

const COLUMN_NAMES: [&str; N_COLUMNS] = [
    "Total Electric Power (kW)",
    "Battery Energy (kWh)",
    "Temperature - Indoor (C)",
    "Battery SOC (-)",
];

fn generate_outcomes(
    num_dwellings: usize,
    num_timesteps: usize,
    start_ts: DateTime<FixedOffset>,
    timestep_minutes: i64,
) -> Vec<DwellingOutcome> {
    let mut outcomes = Vec::with_capacity(num_dwellings);

    let mut times: Vec<String> = Vec::with_capacity(num_timesteps);
    for i in 0..num_timesteps {
        let ts = start_ts + chrono::Duration::minutes(i as i64 * timestep_minutes);
        times.push(ts.to_rfc3339());
    }

    for dwelling_idx in 0..num_dwellings {
        let mut numeric_builders: Vec<Float64Builder> = (0..N_COLUMNS)
            .map(|_| Float64Builder::with_capacity(num_timesteps))
            .collect();

        for t in 0..num_timesteps {
            let seed = (dwelling_idx as u64).wrapping_mul(3_134_847_791)
                ^ (t as u64).wrapping_mul(7_315_641_733);
            let u0 = (seed >> 31) as f64 / (u32::MAX as f64 + 1.0);
            let u1 =
                ((seed.wrapping_mul(6364136223846793005)) >> 33) as f64 / (u64::MAX as f64 + 1.0);
            let u2 =
                ((seed.wrapping_mul(1442695040888963407)) >> 17) as f64 / (u64::MAX as f64 + 1.0);
            let u3 =
                ((seed.wrapping_mul(5282446060444017329)) >> 43) as f64 / (u64::MAX as f64 + 1.0);

            numeric_builders[0].append_value(0.1 + u0 * 9.9);
            numeric_builders[1].append_value(u1 * 0.5);
            numeric_builders[2].append_value(18.0 + u2 * 8.0);
            numeric_builders[3].append_value(u3);
        }

        let mut fields = vec![Field::new("Time", DataType::Utf8, false)];
        for name in &COLUMN_NAMES {
            fields.push(Field::new(*name, DataType::Float64, true));
        }

        let mut arrays: Vec<Arc<dyn arrow::array::Array>> =
            vec![Arc::new(StringArray::from(times.clone()))];
        for mut builder in numeric_builders {
            arrays.push(Arc::new(builder.finish()));
        }

        let schema = Arc::new(Schema::new(fields));
        let batch = RecordBatch::try_new(schema, arrays).expect("valid benchmark batch");
        outcomes.push(outcome_from_batch(batch, 1.0));
    }

    outcomes
}

#[cfg(feature = "observe")]
fn peak_rss_kb() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0 {
        usage.ru_maxrss as u64
    } else {
        0
    }
}

fn bench_aggregation(c: &mut Criterion) {
    let mut group = c.benchmark_group("aggregation");
    group.sample_size(20);
    group.warm_up_time(StdDuration::from_secs(2));
    group.measurement_time(StdDuration::from_secs(5));

    let start = FixedOffset::east_opt(0)
        .expect("UTC offset")
        .with_ymd_and_hms(2021, 1, 1, 0, 0, 0)
        .single()
        .expect("valid start timestamp");

    let combinations: Vec<(usize, i64, &str)> = vec![
        (100, 1, "100x1d"),
        (1000, 1, "1000x1d"),
        (10000, 1, "10000x1d"),
        (100, 365, "100x365d"),
        (1000, 365, "1000x365d"),
    ];

    let resolutions = [
        AggregationResolution::FifteenMin,
        AggregationResolution::Hourly,
    ];

    let timestep_minutes: i64 = 15;

    for (n, days, label) in &combinations {
        let total_minutes = days * 24 * 60;
        let num_timesteps = (total_minutes / timestep_minutes) as usize;
        let outcomes = generate_outcomes(*n, num_timesteps, start, timestep_minutes);
        let total_elements = (*n as u64) * (num_timesteps as u64);

        for &resolution in &resolutions {
            group.throughput(Throughput::Elements(total_elements));

            group.bench_with_input(
                BenchmarkId::from_parameter(format!("{label}_{resolution:?}")),
                &(&outcomes, resolution),
                |b, (outcomes, resolution)| {
                    b.iter_custom(|iters| {
                        let start = Instant::now();

                        #[cfg(feature = "observe")]
                        let rss_before = peak_rss_kb();

                        let mut ret = None;
                        for _ in 0..iters {
                            ret = Some(aggregate(outcomes, *resolution));
                        }
                        criterion::black_box(ret);

                        let elapsed = start.elapsed();

                        #[cfg(feature = "observe")]
                        {
                            let rss_after = peak_rss_kb();
                            tracing::info!(
                                rss_before_kb = rss_before,
                                rss_after_kb = rss_after,
                                rss_delta_kb = rss_after.saturating_sub(rss_before),
                                dwellings = *n,
                                timesteps = num_timesteps,
                                resolution = ?resolution,
                                iters = iters,
                                "aggregation memory observability"
                            );
                        }

                        elapsed
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_aggregation);
criterion_main!(benches);
