//! The scheduled ventilation flow's sampling contract: the schedule samples
//! at the step's time.
//!
//! The `[ventilation] flow_schedule_m3_s` table is hour-resolved data. The
//! schedule build expands it to per-step values by the step's START time
//! (the same rule the event-load columns and the environment's schedule-row
//! addressing follow: the row the step reads is the row of the hour its
//! start falls in), so the fan's signal is the hour's value held across the
//! hour's steps at any timestep: a zero-order hold whose boundaries are the
//! step's own start-time hours.
//!
//! The distinguishing case is a step that STRADDLES an hour boundary: a
//! 900 s step starting 17:45 ends at 18:00. Sampling at the step's time (the
//! start) reads the 17:00 hour's value (the fan OFF); sampling at the step's
//! end would read the 18:00 hour's value (the fan ON). The 900 s run pins
//! the start convention on exactly such a step, and the 3600 s and 900 s
//! runs agree hour-bucket for hour-bucket, the invariance's form for the
//! scheduled flow.
//!
//! Vehicle: Case 650's fixture over one simulated day starting 07:00. The
//! day's first eleven hours are the fan's OFF window (0700-1800 per ASHRAE
//! 140-2017 5.2.2.1.5.2.b), the evening and night its ON window
//! (1800-0700). Starting at 07:00 puts the free-float initialization (the
//! outdoor temperature, both setpoint sides disabled at that hour) a full
//! OFF window away from the first ON step, so every ON step's zone-to-
//! outdoor temperature difference is non-zero and the forced-ventilation
//! gain cannot read zero on the fan's account.
//!
//! Fixture-sized by construction: one simulated day per timestep, no annual
//! runs, one dwelling per run.

use std::fs;
use std::path::PathBuf;

use hares_core::Dwelling;

/// The 650 template at a rewritten timestep and a one-day horizon: the EPW
/// path is absolute (the rewritten copy lives in a temp directory one level
/// deeper, the same fix `timestep_sweep` applies), the start moves to 07:00
/// and the warm-up is disabled (the sampling contract is the step's own
/// read, not the converged annual state).
fn day_toml(time_res_s: u64) -> tempfile::NamedTempFile {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let original = fs::read_to_string(root.join("tests/fixtures/bestest/650.toml"))
        .expect("650.toml must be readable");
    let epw_abs = root
        .join("data/examples/USA_CO_Denver.epw")
        .canonicalize()
        .expect("the Denver EPW exists");
    let rewritten = original
        .replace(
            "../../../data/examples/USA_CO_Denver.epw",
            &epw_abs.display().to_string(),
        )
        .replacen(
            "start_time = \"2024-01-01T00:00:00-07:00\"",
            "start_time = \"2024-01-02T07:00:00-07:00\"",
            1,
        )
        .replacen("time_res = 3600", &format!("time_res = {time_res_s}"), 1)
        .replacen(
            "duration = 31536000",
            &format!("duration = {}", 86_400 / time_res_s * time_res_s),
            1,
        );
    assert_ne!(
        rewritten, original,
        "the template's start_time line must be present to be rewritten"
    );
    let tmp = tempfile::Builder::new()
        .suffix(".toml")
        .tempfile()
        .expect("tempfile must be createable");
    std::fs::write(tmp.path(), &rewritten).expect("write to tempfile must succeed");
    tmp
}

/// The forced-ventilation gain per step, in W, over one simulated day. The
/// gain is `rho × flow × cp × (T_out - T_zone)`: exactly zero when the fan's
/// flow is zero, non-zero on an ON step whose temperature difference is
/// non-zero (the start-time choice above guarantees it).
fn ventilation_gains_w(time_res_s: u64) -> Vec<f64> {
    let toml = day_toml(time_res_s);
    let mut dwelling = Dwelling::from_toml_config_with_reports(toml.path(), Some(Vec::new()))
        .unwrap_or_else(|err| panic!("the {time_res_s} s day builds: {err}"));
    let steps = (86_400 / time_res_s) as usize;
    let mut gains = Vec::with_capacity(steps);
    for _ in 0..steps {
        dwelling
            .step()
            .unwrap_or_else(|err| panic!("the {time_res_s} s day steps: {err}"));
        gains.push(dwelling.thermal_solver().component_gains().ventilation_w);
    }
    assert_eq!(gains.len(), steps);
    gains
}

/// The fan's hour buckets: ON (true) at the step-start hours 18-23 and 0-6,
/// OFF otherwise (ASHRAE 140-2017 5.2.2.1.5.2.b; the standard's "Hour 19
/// through Hour 7" is the hour-ending convention, the intervals
/// [18:00, 19:00) through [06:00, 07:00)).
fn fan_on_at_start_hour(hour: usize) -> bool {
    !(7..=17).contains(&hour)
}

/// The forced-ventilation gains follow the fan's configuration schedule
/// hour-bucket for hour-bucket: an OFF step's gain is exactly zero (the
/// flow is exactly zero), an ON step's is not. A phase slip (sampling at
/// the step's end, or an off-by-one at an hour boundary) moves a step
/// across its bucket and fails the pattern.
fn assert_fan_pattern(gains: &[f64], time_res_s: u64) {
    let steps_per_hour = 3600 / time_res_s;
    let start_hour = 7usize;
    for (step, gain) in gains.iter().enumerate() {
        let hour_of_day = (start_hour + step / steps_per_hour as usize) % 24;
        let on = fan_on_at_start_hour(hour_of_day);
        let label = format!(
            "{time_res_s} s step {step} (start hour {hour_of_day}, fan {})",
            if on { "ON" } else { "OFF" }
        );
        if on {
            assert_ne!(
                *gain, 0.0,
                "{label}: the fan is ON but the forced-ventilation gain is zero"
            );
        } else {
            assert_eq!(
                *gain, 0.0,
                "{label}: the fan is OFF but the forced-ventilation gain is {gain} W"
            );
        }
    }
}

/// The hourly timestep: one step per hour bucket, the fan OFF through the
/// day's first eleven steps (07:00-18:00) and ON through the evening and
/// night (18:00-07:00).
#[test]
fn the_scheduled_fan_follows_its_hours_at_the_hourly_timestep() {
    let gains = ventilation_gains_w(3600);
    assert_eq!(gains.len(), 24);
    assert_fan_pattern(&gains, 3600);
}

/// The 900 s timestep: the same hour buckets, four steps each, and the
/// boundary steps decide the contract: the step starting 17:45 ends at
/// 18:00 and must read OFF (the schedule samples at the step's time), the
/// step starting 18:00 must read ON.
#[test]
fn the_scheduled_fan_follows_its_hours_at_the_900s_timestep() {
    let gains = ventilation_gains_w(900);
    assert_eq!(gains.len(), 96);
    assert_fan_pattern(&gains, 900);
    // The boundary step: 17:45 -> 18:00 is the last OFF step of the day's
    // first OFF window; 18:00 -> 18:15 the first ON step.
    let last_off = 10 * 4 + 3; // hour 17's fourth step
    assert!(!fan_on_at_start_hour(17));
    assert_eq!(gains[last_off], 0.0, "the 17:45 step reads the 17:00 hour");
    assert_ne!(
        gains[last_off + 1],
        0.0,
        "the 18:00 step reads the 18:00 hour"
    );
}
