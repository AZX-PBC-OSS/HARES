//! The per-step diagnostics recording: what a run that requests the
//! diagnostics report pays per step, against a run that does not.
//!
//! Both sides run the same one-day two-zone model at the same verbosity
//! with output on; the only difference is the diagnostics request. The
//! without side pays nothing for the diagnostics (no schema columns, no
//! per-step capture, no post-hoc accumulator); the with side's delta over
//! it is the diagnostics' per-step recording cost: the zone and port
//! reads, the frame writes, and the observe accumulator's record.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use criterion::{Criterion, criterion_group, criterion_main};
use hares_core::{DwellingConfig, SimulationEngine};
use hares_io::{Destination, ReportSpec, Selection, SimulationConfig, SiteLocationOverride};

use hares_core::DwellingSettings;
use hares_io::{DefaultsSource, InputSource, WeatherInput};

fn cz4a_config(diagnostics: bool, output: &Path) -> DwellingConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = root.join("tests/fixtures/parity/cz4a_ashp_hpwh");
    let start = FixedOffset::east_opt(-7 * 3600)
        .expect("offset")
        .with_ymd_and_hms(2023, 1, 1, 0, 0, 0)
        .single()
        .expect("start");
    DwellingConfig {
        hpxml: InputSource::Path(fixture.join("building.xml")),
        schedule: Some(InputSource::Path(fixture.join("schedule.csv"))),
        weather: WeatherInput::Path(fixture.join("weather.epw")),
        defaults: DefaultsSource::directory(root.join("defaults")),
        settings: DwellingSettings::new(
            SimulationConfig {
                start_time: start,
                duration: chrono::Duration::hours(24),
                time_res: chrono::Duration::minutes(1),
                reports: vec![ReportSpec {
                    name: "file".to_string(),
                    selection: Selection::Verbosity(2),
                    frequency: hares_io::Frequency::default(),
                    destination: Destination::Csv {
                        path: output.to_path_buf(),
                        overwrite: true,
                    },
                }],
                diagnostics,
                metrics: false,
                output_chunk_size: 1024,
                setpoint_deadband_c: None,
                master_seed: 0,
                civil_timezone: None,
                site_location: SiteLocationOverride::default(),
                max_consecutive_step_failures: hares_io::DEFAULT_MAX_CONSECUTIVE_STEP_FAILURES,
                health_policy: hares_io::HealthPolicy::default(),
                first_hour_interpolation: hares_io::FirstHour::default(),
                weather_year: hares_io::WeatherYear::default(),
            },
            None,
            1,
            None,
            None,
        ),
    }
}

fn bench_diagnostics_report(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("benchmark directory");
    let engine = SimulationEngine::new();
    let mut group = c.benchmark_group("diagnostics_report");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(6));

    for (label, diagnostics) in [("without", false), ("with", true)] {
        let output = temp.path().join(format!("bench-{label}.csv"));
        group.bench_function(label, |b| {
            b.iter(|| {
                let _ = engine
                    .run(cz4a_config(diagnostics, &output))
                    .expect("diagnostics benchmark run");
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_diagnostics_report);
criterion_main!(benches);
