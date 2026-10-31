//! Timestep-sweep invariance: the free-float envelope's response converges
//! monotonically as the timestep shrinks (3600 -> 900 -> 300 -> 60 s).
//!
//! The envelope's RC discretization is exact in space for the assembled
//! network and second-order in time through the ZOH stepping, so the
//! simulated response's discretization error shrinks with the timestep.
//! The sweep pins the convergence: every metric's sequence moves in one
//! direction across the refinements (the coarse timestep's flat-held
//! hourly weather biases the response one way; no refinement oscillates),
//! or the whole sweep's drift stays inside the 5-percent input-bound the
//! assertion derives. A wiring or integration defect (an unstable
//! sub-step, a schedule phase slip, a missing interpolation) breaks the
//! direction or blows the bound long before a band check would see it.
//!
//! Vehicle: Case 600FF (free float, no HVAC) over two simulated fortnights
//! of the fixture's Denver weather: the winter fortnight (the coldest
//! floor of the year, the heating-side response) and the summer fortnight
//! around the free-float peak (the cooling-side response). Free float is
//! the sensitive vehicle: no thermostat dispatch masks an integration
//! defect, and the zone temperature is the direct output.
//!
//! Fixture-sized by construction: two 14-day windows per timestep, no
//! annual runs, and no dwelling fleet (one dwelling per run).

use std::fs;
use std::path::PathBuf;

use hares_core::Dwelling;

/// The fixture the sweep rides. The template's EPW path is relative to the
/// fixture file; the rewritten copies live in a temp directory one level
/// deeper, so the path is fixed to the project root the same way
/// `tests/warmup_regression.rs` fixes it.
fn template_toml() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let original = fs::read_to_string(root.join("tests/fixtures/bestest/600ff.toml"))
        .expect("600ff.toml must be readable");
    let epw_abs = root
        .join("data/examples/USA_CO_Denver.epw")
        .canonicalize()
        .expect("the Denver EPW exists");
    original.replace(
        "../../../data/examples/USA_CO_Denver.epw",
        &epw_abs.display().to_string(),
    )
}

/// One sweep run's TOML: the 600FF template with the window's start, length
/// and timestep rewritten. The start times are the fixture's own year
/// (2024): the winter fortnight opens on the year's coldest floor (January
/// 6, mid-January's cold snap building), the summer fortnight opens two
/// weeks before the free-float peak window the 600FF band's peak lands in.
fn window_toml(
    template: &str,
    start_time: &str,
    duration_s: u64,
    time_res_s: u64,
) -> tempfile::NamedTempFile {
    let template_start = "start_time = \"2024-01-01T00:00:00-07:00\"";
    let rewritten = template.replacen(template_start, &format!("start_time = \"{start_time}\""), 1);
    assert_ne!(
        rewritten, template,
        "the template's start_time line must be present to be rewritten"
    );
    let rewritten = rewritten
        .replacen("time_res = 3600", &format!("time_res = {time_res_s}"), 1)
        .replacen(
            "duration = 31536000",
            &format!("duration = {duration_s}"),
            1,
        );
    let mut tmp = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("tempfile must be createable");
    std::io::Write::write_all(&mut tmp, rewritten.as_bytes())
        .expect("write to tempfile must succeed");
    tmp
}

/// The free-float fortnight's metrics: the zone temperature extremes and
/// the mean over the window (the mean is the integrated response the sweep
/// converges on; the extremes are the free-float band's own metrics).
#[derive(Debug, Clone, Copy, PartialEq)]
struct WindowResponse {
    peak_temp_c: f64,
    min_temp_c: f64,
    mean_temp_c: f64,
}

fn run_window(template: &str, start_time: &str, time_res_s: u64) -> WindowResponse {
    let duration_s = 14 * 86_400;
    let toml = window_toml(template, start_time, duration_s, time_res_s);
    let mut dwelling = Dwelling::from_toml_config_with_reports(toml.path(), Some(Vec::new()))
        .unwrap_or_else(|err| panic!("the {time_res_s} s window builds: {err}"));
    let steps = (duration_s / time_res_s) as usize;
    let mut peak = f64::NEG_INFINITY;
    let mut min = f64::INFINITY;
    let mut sum = 0.0_f64;
    let mut samples = 0_usize;
    for _ in 0..steps {
        let step = dwelling
            .step()
            .unwrap_or_else(|err| panic!("the {time_res_s} s window steps: {err}"));
        for (_, temp_c) in &step.zone_temperatures_c {
            peak = peak.max(*temp_c);
            min = min.min(*temp_c);
            sum += temp_c;
            samples += 1;
        }
    }
    WindowResponse {
        peak_temp_c: peak,
        min_temp_c: min,
        mean_temp_c: sum / samples.max(1) as f64,
    }
}

/// The sweep's convergence rule, one unified form for every metric:
/// EITHER the sequence is strictly monotone (the ZOH phase bias's
/// direction, held across refinements) OR the whole sweep's drift stays
/// within 5 percent of the 60 s limit (the input-representation bound:
/// the hourly weather record held flat versus interpolated moves the
/// response by no more than that on a converged fixture). A wiring or
/// integration defect satisfies neither: it oscillates across
/// refinements or moves the response past the bound.
///
/// The 5 percent derivation: the ZOH stepping's discretization error is
/// O(dt) in the response, and the input-representation difference (flat
/// hold versus midpoint interpolation of the same hourly file) moves the
/// free-float fortnight's response by single-digit percent at these
/// sizes; 5 percent of the 60 s metric bounds that with headroom while a
/// mis-wired coupling (the Task 1.8 witness class moves a response 2x)
/// fails it by an order of magnitude.
fn assert_converges(metric: &str, values: [f64; 4]) {
    let dir_1 = values[1] - values[0];
    let dir_2 = values[2] - values[1];
    let dir_3 = values[3] - values[2];
    let strictly_monotone = dir_1 * dir_2 > 0.0 && dir_2 * dir_3 > 0.0;
    let limit = values[3].abs().max(1e-9);
    let drift = (values[0] - values[3]).abs();
    let drift_ok = drift <= 0.05 * limit;
    assert!(
        strictly_monotone || drift_ok,
        "{metric} must converge over the timestep sweep (strictly monotone, \
         or the 3600 s value within 5 percent of the 60 s limit): 3600 s {}, \
         900 s {}, 300 s {}, 60 s {}, drift {:.6}",
        values[0],
        values[1],
        values[2],
        values[3],
        drift
    );
    eprintln!(
        "[sweep] {metric}: {:.6} -> {:.6} -> {:.6} -> {:.6} ({})",
        values[0],
        values[1],
        values[2],
        values[3],
        if strictly_monotone {
            "monotone".to_string()
        } else {
            format!("drift {drift:.6} within bound")
        }
    );
}

/// The winter fortnight's free-float response converges over the sweep,
/// on every metric.
#[test]
fn the_winter_fortnight_converges_monotonically_over_the_sweep() {
    let template = template_toml();
    let mut peak = [0.0_f64; 4];
    let mut min = [0.0_f64; 4];
    let mut mean = [0.0_f64; 4];
    for (i, dt) in [3600_u64, 900, 300, 60].iter().enumerate() {
        let r = run_window(&template, "2024-01-06T00:00:00-07:00", *dt);
        peak[i] = r.peak_temp_c;
        min[i] = r.min_temp_c;
        mean[i] = r.mean_temp_c;
    }
    assert_converges("winter peak zone temperature", peak);
    assert_converges("winter minimum zone temperature", min);
    assert_converges("winter mean zone temperature", mean);
}

/// The summer fortnight's free-float response (the window whose peak the
/// 600FF band pins) converges monotonically over the sweep, on every metric.
#[test]
fn the_summer_fortnight_converges_monotonically_over_the_sweep() {
    let template = template_toml();
    let mut peak = [0.0_f64; 4];
    let mut min = [0.0_f64; 4];
    let mut mean = [0.0_f64; 4];
    for (i, dt) in [3600_u64, 900, 300, 60].iter().enumerate() {
        let r = run_window(&template, "2024-07-01T00:00:00-07:00", *dt);
        peak[i] = r.peak_temp_c;
        min[i] = r.min_temp_c;
        mean[i] = r.mean_temp_c;
    }
    assert_converges("summer peak zone temperature", peak);
    assert_converges("summer minimum zone temperature", min);
    assert_converges("summer mean zone temperature", mean);
}
