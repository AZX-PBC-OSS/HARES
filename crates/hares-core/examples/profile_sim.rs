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
//! and its cost lands in the `other` residual — use the phase shares RELATIVE to each
//! other, and re-baseline absolute times against a non-profiling build.

use std::path::PathBuf;
use std::time::Instant;

use hares_core::dwelling::{Dwelling, DwellingConfig};
use hares_io::SimulationConfig;

fn config(days: i64, fixture: &PathBuf, weather: &PathBuf) -> DwellingConfig {
    // SimulationConfig carries serde defaults for every field we do not name here.
    let sim: SimulationConfig = serde_json::from_str(&format!(
        r#"{{
            "start_time": "2018-01-01T00:00:00-07:00",
            "duration": {},
            "time_res": 900,
            "output_verbosity": 1,
            "write_output": false,
            "master_seed": 0
        }}"#,
        days * 24 * 3600
    ))
    .unwrap();

    DwellingConfig {
        hpxml_path: fixture.join("home.xml"),
        schedule_path: fixture.join("in.schedules.csv"),
        weather_path: weather.clone(),
        sim_config: sim,
        bldg_id: 7,
        defaults_path: None,
        overrides: None,
        initialization_duration: None,
        resample_overrides: None,
        patches: None,
    }
}

#[cfg(feature = "profiling")]
fn main() {
    let days: i64 = std::env::args().nth(1).unwrap_or_else(|| "45".to_string()).parse().unwrap();
    let reps: usize = std::env::args().nth(2).unwrap_or_else(|| "3".to_string()).parse().unwrap();

    // crates/hares-core -> crates -> repo root
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = root.parent().unwrap().parent().unwrap().to_path_buf();
    let fixture = root.join("tests/fixtures/resstock/2025.1/bldg0000007");
    let weather = root.join("tests/fixtures/resstock/2025.1/weather/G0100590_2018.csv");

    // One warm run so allocator and cache state are steady before anything is timed.
    let mut warm = Dwelling::from_config(config(days, &fixture, &weather)).unwrap();
    warm.simulate().unwrap();

    let mut acc = DwellingProfilingSummary::default();
    let t0 = Instant::now();
    for _ in 0..reps {
        let mut dw = Dwelling::from_config(config(days, &fixture, &weather)).unwrap();
        dw.simulate().unwrap();
        let p = dw.profiling_summary();
        acc.envelope_solve += p.envelope_solve;
        acc.hvac += p.hvac;
        acc.water_heater += p.water_heater;
        acc.schedule_load += p.schedule_load;
        acc.io += p.io;
        acc.other += p.other;
    }
    let wall = t0.elapsed();
    eprintln!(
        "wall {wall:.2?} for {reps} x {days}-day runs (plus one warm-up); \
         per-phase means per run:"
    );
    for (name, d) in [
        ("envelope_solve", acc.envelope_solve),
        ("schedule_load", acc.schedule_load),
        ("hvac", acc.hvac),
        ("io", acc.io),
        ("other", acc.other),
        ("water_heater", acc.water_heater),
    ] {
        eprintln!("  {name:15} {:>8.1} ms/run", d.as_secs_f64() * 1e3 / reps as f64);
    }
}

#[cfg(not(feature = "profiling"))]
fn main() {
    eprintln!("build with --features profiling (see the doc comment)");
}

#[cfg(feature = "profiling")]
use hares_core::dwelling::DwellingProfilingSummary;
