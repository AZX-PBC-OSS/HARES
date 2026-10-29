//! Self-contained profiling harness for the full dwelling step loop.
//!
//! Native profilers (perf, py-spy --native) need kernel PMUs or ptrace, neither of which
//! is reliably available (WSL2, containers). This example uses the engine's OWN
//! `profiling` feature instead:
//!
//!     cargo run --release -p hares-core --features profiling --example profile_sim -- [days] [reps]
//!
//! NOTE the caveat recorded in the calibrator's Optimization_Register and issue #31: the
//! per-step VmHWM sampling makes the profiling build several times slower than release
//! and its cost lands inside whichever phase pays for it — use the phase shares RELATIVE
//! to each other, and re-baseline absolute times against a non-profiling build.

#[cfg(feature = "profiling")]
mod harness {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use hares_core::dwelling::{
        Dwelling, DwellingConfig, DwellingProfilingSummary, DwellingSettings,
    };
    use hares_io::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};

    fn config(days: i64, root: &Path, fixture: &Path, weather: &Path) -> DwellingConfig {
        // SimulationConfig carries serde defaults for every field we do not name
        // here. With no reports the run records nothing: the step loop is the
        // whole cost.
        let sim_toml = format!(
            r#"
start_time = "2018-01-01T00:00:00-07:00"
duration = {}
time_res = 900
master_seed = 0
"#,
            days * 24 * 3600
        );
        let sim = SimulationConfig::from_toml(&sim_toml).unwrap();

        DwellingConfig {
            hpxml: InputSource::Path(fixture.join("home.xml")),
            schedule: Some(InputSource::Path(fixture.join("in.schedules.csv"))),
            weather: WeatherInput::Path(weather.to_path_buf()),
            defaults: DefaultsSource::directory(root.join("python/hares/defaults")),
            settings: DwellingSettings::new(sim, None, 7, None),
        }
    }

    pub fn main() {
        let days: i64 = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "45".to_string())
            .parse()
            .unwrap();
        let reps: usize = std::env::args()
            .nth(2)
            .unwrap_or_else(|| "3".to_string())
            .parse()
            .unwrap();

        // crates/hares-core -> crates -> repo root
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = root.parent().unwrap().parent().unwrap().to_path_buf();
        let fixture = root.join("tests/fixtures/resstock/2025.1/bldg0000007");
        let weather = root.join("tests/fixtures/resstock/2025.1/weather/G0100590_2018.csv");

        // One warm run so allocator and cache state are steady before anything is timed.
        let mut warm = Dwelling::from_config(config(days, &root, &fixture, &weather)).unwrap();
        warm.simulate().unwrap();

        let mut acc = DwellingProfilingSummary::default();
        let t0 = Instant::now();
        for _ in 0..reps {
            let mut dw = Dwelling::from_config(config(days, &root, &fixture, &weather)).unwrap();
            dw.simulate().unwrap();
            let p = dw.profiling_summary();
            acc.environment += p.environment;
            acc.control += p.control;
            acc.ideal_capacity += p.ideal_capacity;
            acc.actors += p.actors;
            acc.dispatch += p.dispatch;
            acc.equipment += p.equipment;
            acc.envelope += p.envelope;
            acc.invariants += p.invariants;
            acc.state_snapshot += p.state_snapshot;
            acc.output += p.output;
            acc.accounting += p.accounting;
            acc.step_total += p.step_total;
        }
        let wall = t0.elapsed();
        eprintln!(
            "wall {wall:.2?} for {reps} x {days}-day runs (plus one warm-up); \
             per-phase means per run:"
        );
        for (name, d) in [
            ("environment", acc.environment),
            ("control", acc.control),
            ("ideal_capacity", acc.ideal_capacity),
            ("actors", acc.actors),
            ("dispatch", acc.dispatch),
            ("equipment", acc.equipment),
            ("envelope", acc.envelope),
            ("invariants", acc.invariants),
            ("state_snapshot", acc.state_snapshot),
            ("output", acc.output),
            ("accounting", acc.accounting),
            ("step_total", acc.step_total),
        ] {
            eprintln!(
                "  {name:15} {:>8.1} ms/run",
                d.as_secs_f64() * 1e3 / reps as f64
            );
        }
    }
}

fn main() {
    #[cfg(feature = "profiling")]
    harness::main();
    #[cfg(not(feature = "profiling"))]
    eprintln!("build with --features profiling (see the doc comment)");
}
