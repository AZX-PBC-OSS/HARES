//! The run metrics' cost: what a run that requests the metrics pays per
//! step, against a run that does not.
//!
//! Both sides run the same one-day two-zone model at the same verbosity
//! with the same memory report; the only difference is the metrics
//! request. The without side builds no metrics input plan and no
//! accumulator and pays nothing for the metrics; the with side's delta
//! over it is the metrics' per-step cost: the accumulator's reads from the
//! row plan's producer slots and its arithmetic.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use criterion::{Criterion, criterion_group, criterion_main};
use hares_core::{DwellingConfig, SimulationEngine};
use hares_io::{Destination, ReportSpec, Selection, SimulationConfig, SiteLocationOverride};

fn cz4a_config(metrics: bool) -> DwellingConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let start = FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2024, 2, 27, 0, 0, 0)
        .single()
        .expect("start");
    DwellingConfig {
        hpxml_path: fixture.join("building.xml"),
        schedule_path: Some(fixture.join("schedule.csv")),
        weather_path: fixture.join("weather.epw"),
        sim_config: SimulationConfig {
            start_time: start,
            duration: chrono::Duration::hours(24),
            time_res: chrono::Duration::minutes(15),
            reports: vec![ReportSpec {
                name: "frame".to_string(),
                selection: Selection::Verbosity(2),
                frequency: hares_io::Frequency::default(),
                destination: Destination::Memory,
            }],
            diagnostics: false,
            metrics,
            output_chunk_size: 1024,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: SiteLocationOverride::default(),
            max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
        },
        defaults_path: Some(root.join("defaults")),
        overrides: None,
        bldg_id: 1,
        initialization_duration: None,
        patches: None,
    }
}

fn bench_metrics_request(c: &mut Criterion) {
    let engine = SimulationEngine::new();
    let mut group = c.benchmark_group("metrics_request");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(6));

    for (label, requested) in [("without", false), ("with", true)] {
        group.bench_function(label, |b| {
            b.iter(|| {
                let _ = engine
                    .run(cz4a_config(requested))
                    .expect("metrics benchmark run");
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_metrics_request);
criterion_main!(benches);
