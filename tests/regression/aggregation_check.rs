//! Fleet aggregation regression sub-suite.
//!
//! The one test that replaces both former halves (the synthetic
//! `aggregation::aggregate` cases and the fleet integration check): four
//! `base.xml` homes run through `Fleet::simulate` with weights 1, 2, 5
//! and 1, the last given a nonexistent schedule path so it fails, and the
//! result's properties checked: the failed home is listed with its reason
//! and excluded from `coverage.weight_ok`, a `Total Electric Power (kW)`
//! cell is the weighted sum of the three homes' period means, a zone
//! temperature (`C`) cell is their weighted mean, and the row count is
//! the period count.

use std::collections::BTreeMap;

use arrow::array::Float64Array;
use arrow::record_batch::RecordBatch;
use chrono::{Duration, Timelike};
use hares_fleet::{Fleet, FleetReport};
use hares_io::{Frequency, Selection};

use super::helpers;

/// One home's Timestep memory report, run standalone at the same
/// building id, seed and inputs: the source of the hand-computed period
/// means.
fn standalone_home_periods(
    bldg_id: i64,
    seed: u64,
    schedule_path: std::path::PathBuf,
    weather_path: std::path::PathBuf,
    duration: Duration,
    output_dir: &std::path::Path,
) -> BTreeMap<i64, Vec<f64>> {
    let mut config = helpers::build_dwelling_config(
        output_dir,
        bldg_id,
        schedule_path,
        weather_path,
        duration,
        seed,
    );
    config.sim_config.reports = vec![hares_io::ReportSpec {
        name: "frame".to_string(),
        selection: Selection::Verbosity(2),
        frequency: Frequency::Timestep,
        destination: hares_io::Destination::Memory,
    }];
    let mut dwelling = hares_core::Dwelling::from_config(config).expect("the home builds");
    let sim_config = dwelling.sim_config().clone();
    let result = hares_core::SimulationEngine::new()
        .run_dwelling(&mut dwelling, &sim_config)
        .expect("the home runs");

    // The power and the indoor-temperature columns, in step order, keyed
    // by the hour bucket the fleet's fold keys (the naive local wall
    // clock floored to the hour, read as UTC seconds). Per bucket: the
    // step-order sums and the step count; the period means divide once.
    let batches: Vec<RecordBatch> = result
        .reports
        .iter()
        .flat_map(|report| report.batches().to_vec())
        .collect();
    let schema = batches
        .first()
        .expect("the memory report retained batches")
        .schema_ref()
        .clone();
    let power_slot = schema
        .fields()
        .iter()
        .position(|field| field.name() == "Total Electric Power (kW)")
        .expect("the power column exists at tier 2");
    let temp_slot = schema
        .fields()
        .iter()
        .position(|field| field.name() == "Temperature - Indoor (C)")
        .expect("the indoor temperature column exists at tier 2");
    let time_slot = 0usize;
    let utc_offset_s: i64 = schema
        .metadata()
        .get("hares:utc_offset_s")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);

    let mut periods: BTreeMap<i64, (Vec<f64>, u32)> = BTreeMap::new();
    for batch in &batches {
        let times = batch
            .column(time_slot)
            .as_any()
            .downcast_ref::<arrow::array::TimestampMillisecondArray>()
            .expect("the timestamp Time column");
        let power = batch
            .column(power_slot)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column");
        let temp = batch
            .column(temp_slot)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("a Float64 column");
        for row in 0..batch.num_rows() {
            let naive = chrono::DateTime::from_timestamp_millis(times.value(row))
                .expect("a valid stamp")
                .naive_utc();
            // The bucket key: the row's local wall clock floored to the
            // hour (the reducer's period start), read as UTC seconds,
            // minus the home's offset.
            let floored = naive
                .with_minute(0)
                .and_then(|n| n.with_second(0))
                .and_then(|n| n.with_nanosecond(0))
                .expect("a valid hour floor");
            let bucket = floored.and_utc().timestamp() - utc_offset_s;
            let (sums, count) = periods.entry(bucket).or_insert_with(|| (vec![0.0, 0.0], 0));
            sums[0] += power.value(row);
            sums[1] += temp.value(row);
            *count += 1;
        }
    }
    // The period means: the step-order sums divided once by the step
    // count, the same division the fleet's reducer performs.
    periods
        .into_iter()
        .map(|(bucket, (sums, count))| {
            let count = f64::from(count);
            (bucket, vec![sums[0] / count, sums[1] / count])
        })
        .collect()
}

pub fn run_aggregation_check() -> Result<(), Vec<String>> {
    let mut failures = Vec::new();

    let scratch = tempfile::tempdir().expect("create scratch directory");
    let schedule_path = scratch.path().join("schedule.csv");
    let weather_path = scratch.path().join("weather.epw");
    helpers::write_schedule_csv(&schedule_path);
    helpers::write_weather_epw(&weather_path);

    let duration = Duration::hours(1);
    let mut configs: Vec<_> = (0..3)
        .map(|idx| {
            helpers::build_dwelling_config(
                scratch.path(),
                idx as i64 + 1,
                schedule_path.clone(),
                weather_path.clone(),
                duration,
                idx as u64,
            )
        })
        .collect();
    // The fourth home's schedule does not exist: the home fails.
    configs.push(hares_core::DwellingConfig {
        schedule_path: Some(scratch.path().join("no-such-schedule.csv")),
        ..helpers::build_dwelling_config(
            scratch.path(),
            4,
            schedule_path.clone(),
            weather_path.clone(),
            duration,
            3,
        )
    });

    let fleet = Fleet::from_buildings(configs)
        .map_err(|err| vec![format!("fleet construction: {err}")])?
        .with_sample_weights(vec![1.0, 2.0, 5.0, 1.0])
        .map_err(|err| vec![format!("sample weights: {err}")])?;
    let report = FleetReport {
        selection: Selection::Verbosity(2),
        frequency: Frequency::Hourly,
        health_policy: hares_io::HealthPolicy::default(),
    };
    let result = match fleet.simulate(&report, 1, std::num::NonZeroUsize::new(64).expect("64")) {
        Ok(result) => result,
        Err(err) => return Err(vec![format!("fleet simulate: {err}")]),
    };

    // The failed home is listed with its reason, and excluded from the
    // fold-eligible weight: the three good homes carry the BEopt
    // fixture's own HPXML provenance warning (they are flagged, not ok),
    // so their weight sits in weight_flagged; the failed home's weight is
    // in neither.
    let failed = &result.homes[3];
    if !matches!(failed.status, hares_fleet::SimStatus::Failed(_)) {
        failures.push(format!("home 4 must be failed, got {:?}", failed.status));
    } else if failed
        .reason
        .as_deref()
        .is_none_or(|reason| reason.is_empty())
    {
        failures.push("the failed home's reason is empty".to_string());
    }
    let good_weight = 1.0 + 2.0 + 5.0;
    if result.coverage.weight_ok + result.coverage.weight_flagged != good_weight {
        failures.push(format!(
            "coverage.weight_ok {} + weight_flagged {} != {good_weight} (the failed home's weight excluded)",
            result.coverage.weight_ok, result.coverage.weight_flagged
        ));
    }
    if result.coverage.weight_failed != 1.0 {
        failures.push(format!(
            "coverage.weight_failed {} != 1.0",
            result.coverage.weight_failed
        ));
    }
    if result.n_succeeded != 3 {
        failures.push(format!("n_succeeded {} != 3", result.n_succeeded));
    }

    // The row count is the period count: one hourly period.
    if result.aggregate.num_rows() != 1 {
        failures.push(format!(
            "the aggregate's row count {} != 1 (the run's period count)",
            result.aggregate.num_rows()
        ));
        return Err(failures);
    }

    // A power cell is the weighted sum of the three homes' period means;
    // a temperature cell is their weighted mean. The homes' own period
    // means come from standalone runs at the same building ids.
    let period_start = result
        .aggregate
        .column(0)
        .as_any()
        .downcast_ref::<arrow::array::StringArray>()
        .expect("the Time column")
        .value(0)
        .to_string();
    let bucket = chrono::DateTime::parse_from_rfc3339(&period_start)
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|err| {
            failures.push(format!("the aggregate's Time label: {err}"));
            0
        });

    let homes: [(i64, f64); 3] = [(1, 1.0), (2, 2.0), (3, 5.0)];
    let mut weighted_power = 0.0;
    let mut weighted_temp = 0.0;
    let mut total_weight = 0.0;
    for (bldg_id, weight) in homes {
        let seed = u64::try_from(bldg_id - 1).expect("seed");
        let periods = standalone_home_periods(
            bldg_id,
            seed,
            schedule_path.clone(),
            weather_path.clone(),
            duration,
            scratch.path(),
        );
        let means = periods
            .get(&bucket)
            .expect("the home's hourly period covers the fleet's row");
        weighted_power += means[0] * weight;
        weighted_temp += means[1] * weight;
        total_weight += weight;
    }
    let expected_temp = weighted_temp / total_weight;

    let cell = |name: &str| -> Option<f64> {
        let slot = result
            .aggregate
            .schema_ref()
            .fields()
            .iter()
            .position(|field| field.name() == name);
        let slot = slot?;
        result
            .aggregate
            .column(slot)
            .as_any()
            .downcast_ref::<Float64Array>()
            .map(|values| values.value(0))
    };
    if cell("Total Electric Power (kW)")
        .is_none_or(|power| power.to_bits() != weighted_power.to_bits())
    {
        failures.push(format!(
            "the power cell is not the weighted sum of the homes' period means ({weighted_power})"
        ));
    }
    if cell("Temperature - Indoor (C)").is_none_or(|temp| temp.to_bits() != expected_temp.to_bits())
    {
        failures.push(format!(
            "the temperature cell is not the weighted mean of the homes' period means ({expected_temp})"
        ));
    }

    if failures.is_empty() {
        eprintln!("[aggregation_check] PASS");
        Ok(())
    } else {
        Err(failures)
    }
}
