mod cases;
mod reference_bands;

use std::collections::BTreeMap;

use cases::{BestestCase, core_cases, extended_cases};
use hares_core::Dwelling;
use reference_bands::{BestestMetric, ReferenceBand, core_reference_bands};

#[derive(Debug, Clone)]
struct CaseObservation {
    case_id: &'static str,
    values: BTreeMap<BestestMetric, f64>,
}

#[derive(Debug, Clone)]
struct BandCheck {
    case_id: &'static str,
    metric: BestestMetric,
    value: f64,
    min: f64,
    max: f64,
    passed: bool,
}

fn run_single_case(case: &BestestCase) {
    let observation = run_case(case);
    let bands = core_reference_bands(case.id);
    if bands.is_empty() {
        panic!("case={} has no reference bands", case.id);
    }

    eprintln!(
        "[bestest] case={} tier={:?} {}",
        case.id, case.tier, case.description
    );
    let mut failures = Vec::new();
    for check in evaluate_bands(&observation, &bands) {
        let status = if check.passed { "PASS" } else { "FAIL" };
        eprintln!(
            "  {} metric={} value={:.6} ref_min={:.6} ref_max={:.6}",
            status, check.metric, check.value, check.min, check.max
        );
        if !check.passed {
            failures.push(format!(
                "case={} metric={} value={:.6} outside [{:.6}, {:.6}]",
                check.case_id, check.metric, check.value, check.min, check.max
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "BESTEST band violations:\n{}",
        failures.join("\n")
    );
}

#[test]
fn bestest_case_600() {
    let case = core_cases().into_iter().find(|c| c.id == "600").unwrap();
    run_single_case(&case);
}

#[test]
fn bestest_case_900() {
    let case = core_cases().into_iter().find(|c| c.id == "900").unwrap();
    run_single_case(&case);
}

#[test]
fn bestest_case_600ff() {
    let case = core_cases().into_iter().find(|c| c.id == "600FF").unwrap();
    run_single_case(&case);
}

#[test]
fn bestest_case_900ff() {
    let case = core_cases().into_iter().find(|c| c.id == "900FF").unwrap();
    run_single_case(&case);
}

#[test]
fn bestest_case_640() {
    let case = core_cases().into_iter().find(|c| c.id == "640").unwrap();
    run_single_case(&case);
}

#[test]
fn bestest_extended_cases_are_tracked() {
    let cases = extended_cases();
    assert!(
        !cases.is_empty(),
        "expected non-empty extended BESTEST case set"
    );
    for case in cases {
        assert!(
            case.fixture_path().exists(),
            "missing extended BESTEST fixture: {}",
            case.fixture_path().display()
        );
    }
}

fn run_case(case: &BestestCase) -> CaseObservation {
    let t0 = std::time::Instant::now();
    let mut dwelling =
        Dwelling::from_toml_config_with_write_output(&case.fixture_path(), Some(false))
            .unwrap_or_else(|err| panic!("failed to load BESTEST case {}: {err}", case.id));
    eprintln!("[bestest] case={} loaded in {:?}", case.id, t0.elapsed());
    let t1 = std::time::Instant::now();
    let results = dwelling
        .simulate()
        .unwrap_or_else(|err| panic!("simulation failed for BESTEST case {}: {err}", case.id));
    eprintln!(
        "[bestest] case={} simulated {} steps in {:?}",
        case.id,
        results.steps.len(),
        t1.elapsed()
    );

    let dt_hours = case.timestep_seconds as f64 / 3600.0;
    let mut max_temp = f64::NEG_INFINITY;
    let mut min_temp = f64::INFINITY;
    let mut heating_kwh = 0.0;
    let mut cooling_kwh = 0.0;

    for step in &results.steps {
        for (_, temp_c) in &step.zone_temperatures_c {
            max_temp = max_temp.max(*temp_c);
            min_temp = min_temp.min(*temp_c);
        }
        heating_kwh += step.hvac_heating_w / 1000.0 * dt_hours;
        cooling_kwh += step.hvac_cooling_w / 1000.0 * dt_hours;
    }

    let mut values = BTreeMap::new();
    if max_temp.is_finite() {
        values.insert(BestestMetric::PeakZoneTempC, max_temp);
    }
    if min_temp.is_finite() {
        values.insert(BestestMetric::MinZoneTempC, min_temp);
    }
    values.insert(BestestMetric::AnnualHeatingLoadKwh, heating_kwh);
    values.insert(BestestMetric::AnnualCoolingLoadKwh, cooling_kwh);
    values.insert(BestestMetric::AnnualHeatingEnergyKwh, heating_kwh);

    CaseObservation {
        case_id: case.id,
        values,
    }
}

#[cfg(feature = "observe")]
#[test]
#[ignore = "targeted runtime tracing helper for BESTEST physics debugging"]
fn debug_bestest_600ff_observe_peak_terms() {
    let case = core_cases().into_iter().find(|c| c.id == "600FF").unwrap();
    let mut dwelling =
        Dwelling::from_toml_config_with_write_output(&case.fixture_path(), Some(false))
            .unwrap_or_else(|err| panic!("failed to load BESTEST case {}: {err}", case.id));
    let horizon_steps = 24;
    dwelling.enable_observer(horizon_steps + 1);
    for _ in 0..horizon_steps {
        dwelling.step().unwrap_or_else(|err| {
            panic!("simulation step failed for BESTEST case {}: {err}", case.id)
        });
    }
    let snapshots = dwelling.drain_observations();
    assert_eq!(
        snapshots.len(),
        horizon_steps,
        "expected full short-horizon capture"
    );

    let mut max_temp_c = f64::NEG_INFINITY;
    let mut max_temp_idx = 0usize;
    for (idx, snap) in snapshots.iter().enumerate() {
        let step_max = snap
            .phases
            .post_zone_update
            .as_ref()
            .expect("post_zone_update capture missing")
            .zone_temps_c
            .iter()
            .map(|(_, t)| *t)
            .fold(f64::NEG_INFINITY, f64::max);
        if step_max > max_temp_c {
            max_temp_c = step_max;
            max_temp_idx = idx;
        }
    }

    let snap = &snapshots[max_temp_idx];
    let env = snap
        .phases
        .post_environment
        .as_ref()
        .expect("post_environment capture missing");
    let gains = &snap
        .phases
        .post_solvers
        .as_ref()
        .expect("post_solvers capture missing")
        .envelope_gains;

    let (max_window_idx, max_window_w) = snapshots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            Some((
                i,
                s.phases
                    .post_solvers
                    .as_ref()?
                    .envelope_gains
                    .window_solar_w,
            ))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("missing post_solvers capture");

    eprintln!(
        "[observe] case=600FF peak_step={} peak_temp_c={:.3} time={} outdoor_c={:.3} ghi={:.1} dni={:.1} dhi={:.1}",
        max_temp_idx,
        max_temp_c,
        snap.timestamp,
        env.outdoor_temp_c,
        env.ghi_w_m2,
        env.dni_w_m2,
        env.dhi_w_m2
    );
    eprintln!(
        "[observe] gains@peak window_solar_w={:.1} opaque_solar_lwr_w={:.1} infiltration_w={:.1} internal_gain_w={:.1} hvac_heating_w={:.1} hvac_cooling_w={:.1} port_sensible_w={:.1}",
        gains.window_solar_w,
        gains.opaque_solar_lwr_w,
        gains.infiltration_w,
        gains.internal_gain_w,
        gains.hvac_heating_w,
        gains.hvac_cooling_w,
        gains.port_sensible_w
    );
    eprintln!(
        "[observe] max_window_solar step={} value_w={:.1}",
        max_window_idx, max_window_w
    );
}

fn evaluate_bands(observation: &CaseObservation, bands: &[ReferenceBand]) -> Vec<BandCheck> {
    let mut checks = Vec::new();
    for band in bands {
        let Some(value) = observation.values.get(&band.metric).copied() else {
            checks.push(BandCheck {
                case_id: observation.case_id,
                metric: band.metric,
                value: f64::NAN,
                min: band.min,
                max: band.max,
                passed: false,
            });
            continue;
        };
        checks.push(BandCheck {
            case_id: band.case_id,
            metric: band.metric,
            value,
            min: band.min,
            max: band.max,
            passed: band.contains(value),
        });
    }
    checks
}
