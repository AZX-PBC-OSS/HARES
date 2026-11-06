//! The strict Annex B8 band harness: a separately selected annual BESTEST
//! surface that asserts the ASHRAE 140-2023 Annex B8 bands directly, not
//! the drift locks.
//!
//! Selection contract. The drift-lock harness (`bestest` target) keeps
//! every out-of-band metric pinned at its measured baseline so silent
//! movement is impossible; it is explicitly not a conformance claim. This
//! target is the band's own witness: each test asserts the metric against
//! the standard's band and fails while the metric sits outside it. The
//! tests are `#[ignore]`d with the reason recorded on the attribute (the
//! machine-readable form): the out-of-band residuals they assert are the
//! residual-attribution study's measured object, the study moves no
//! production number, and an always-failing target would break every
//! unrelated gate run. They run when deliberately selected:
//! `cargo nextest -p hares-core --test bestest_strict_b8 --run-ignored only`.
//! The selection never replaces or weakens the drift locks or the general
//! regression harness: those targets are untouched and keep running.
//!
//! RED state at the parent `5d5a73d0` (the post-gains-split baseline,
//! measured 2026-11-05): exactly four of the five asserted metrics sit
//! outside their bands - Case 395 annual heating (4385.626148 against the
//! 4565 floor), Case 900 annual heating (1342.637964 against the 1379
//! floor), Case 600FF peak zone temperature (70.248155 against the 68.4
//! ceiling), Case 900FF minimum zone temperature (2.379882 against the
//! 2.2 ceiling) - and Case 600's annual heating load verifies its
//! closure in band (4033.911883 in [3993, 4504], the gains-split's
//! withdrawn baseline). The values are the re-measure the drift-lock
//! table records; the strict harness prints them at full precision on
//! every run.
//!
//! Band provenance is the standard's, unchanged: ANSI/ASHRAE 140-2023
//! Annex B8 Tables B8-1, B8-2 and B8-5 (the rows pinned in
//! `bestest/reference_bands.rs`, sourced from the public
//! Std140_TF_Results.pdf report). No band is widened, narrowed, or
//! re-sourced here.

#[path = "bestest/cases.rs"]
mod cases;

#[path = "bestest/reference_bands.rs"]
mod reference_bands;

#[cfg(feature = "experimental-scriptf")]
#[path = "bestest_strict_b8/residual_arms.rs"]
mod residual_arms;
use cases::{BestestCase, in_depth_cases};
use hares_core::Dwelling;
use reference_bands::{BestestMetric, ReferenceBand, core_reference_bands};

/// One strict selection entry: the case, the metric, and the band that
/// metric must sit inside.
struct StrictCheck {
    case_id: &'static str,
    metric: BestestMetric,
}

/// The five asserted metrics: the four out-of-band residuals plus Case
/// 600's heating closure. Every other metric of every case stays with the
/// drift-lock harness and the bands it already runs against; 600 cooling's
/// post-gains-split residual is P1's own record (the closure-hypothesis
/// kill the drift-lock table carries), not this selection's.
fn strict_checks() -> Vec<StrictCheck> {
    use BestestMetric::*;
    vec![
        StrictCheck {
            case_id: "600",
            metric: AnnualHeatingLoadKwh,
        },
        StrictCheck {
            case_id: "395",
            metric: AnnualHeatingLoadKwh,
        },
        StrictCheck {
            case_id: "900",
            metric: AnnualHeatingLoadKwh,
        },
        StrictCheck {
            case_id: "600FF",
            metric: PeakZoneTempC,
        },
        StrictCheck {
            case_id: "900FF",
            metric: MinZoneTempC,
        },
    ]
}

/// The annual run behind a strict check: the drift-lock harness's own
/// metric definitions (the step loop's HVAC powers and the zone
/// temperature extremes), recorded at full precision.
struct StrictObservation {
    heating_kwh: f64,
    cooling_kwh: f64,
    peak_temp_c: f64,
    min_temp_c: f64,
}

fn run_strict(case: &BestestCase) -> StrictObservation {
    let mut dwelling =
        Dwelling::from_toml_config_with_reports(&case.fixture_path(), Some(Vec::new()))
            .unwrap_or_else(|err| panic!("failed to load BESTEST case {}: {err}", case.id));
    let total_steps = dwelling.clock.total_steps();
    let dt_hours = case.timestep_seconds as f64 / 3600.0;
    let mut heating_kwh = 0.0;
    let mut cooling_kwh = 0.0;
    let mut peak_temp_c = f64::NEG_INFINITY;
    let mut min_temp_c = f64::INFINITY;
    for _ in 0..total_steps {
        let step = dwelling
            .step()
            .unwrap_or_else(|err| panic!("simulation failed for BESTEST case {}: {err}", case.id));
        for (_, temp_c) in &step.zone_temperatures_c {
            peak_temp_c = peak_temp_c.max(*temp_c);
            min_temp_c = min_temp_c.min(*temp_c);
        }
        heating_kwh += step.hvac_heating_w / 1000.0 * dt_hours;
        cooling_kwh += step.hvac_cooling_w / 1000.0 * dt_hours;
    }
    StrictObservation {
        heating_kwh,
        cooling_kwh,
        peak_temp_c,
        min_temp_c,
    }
}

impl StrictObservation {
    fn metric(&self, metric: BestestMetric) -> f64 {
        match metric {
            BestestMetric::AnnualHeatingLoadKwh => self.heating_kwh,
            BestestMetric::AnnualCoolingLoadKwh => self.cooling_kwh,
            BestestMetric::AnnualHeatingEnergyKwh => self.heating_kwh,
            BestestMetric::PeakZoneTempC => self.peak_temp_c,
            BestestMetric::MinZoneTempC => self.min_temp_c,
        }
    }
}

/// The band for the check's own case and metric: the standard's pinned
/// row, found, not re-declared here.
fn band_for(case_id: &str, metric: BestestMetric) -> ReferenceBand {
    let bands = core_reference_bands(case_id);
    assert!(
        !bands.is_empty(),
        "case {case_id} has no pinned reference bands"
    );
    bands
        .into_iter()
        .find(|band| band.metric == metric)
        .unwrap_or_else(|| panic!("case {case_id} pins no band for metric {metric}"))
}

/// Runs one strict check: the annual run, the band containment, and the
/// full-precision print the record reads. The failure message carries the
/// value, the band, and the signed distance so the out-of-band state is
/// the report's own evidence.
fn assert_strict(check: &StrictCheck) {
    let registry: Vec<BestestCase> = cases::core_cases()
        .into_iter()
        .chain(in_depth_cases())
        .collect();
    let case = registry
        .iter()
        .find(|c| c.id == check.case_id)
        .unwrap_or_else(|| panic!("case {} is in the BESTEST case registry", check.case_id));
    let observation = run_strict(case);
    let band = band_for(check.case_id, check.metric);
    let value = observation.metric(check.metric);
    let distance = if band.contains(value) {
        0.0
    } else if value < band.min {
        value - band.min
    } else {
        value - band.max
    };
    eprintln!(
        "[strict-b8] case={} metric={} value={:?} band=[{:?}, {:?}] band_distance={:+.6}",
        check.case_id, check.metric, value, band.min, band.max, distance
    );
    assert!(
        band.contains(value),
        "strict Annex B8: case {} metric {} value {:?} sits outside its band \
         [{:?}, {:?}] by {:+.6}; the residual is the attribution study's \
         object: no band widens and no drift lock moves for it",
        check.case_id,
        check.metric,
        value,
        band.min,
        band.max,
        distance
    );
}

#[ignore = "the strict Annex B8 band selection: the out-of-band residuals it asserts \
     are the residual-attribution study's measured object and the study moves \
     no production number, so the tests stay red until a separately evidenced \
     physics row closes each residual; select with --run-ignored only"]
#[test]
fn strict_b8_case_600_annual_heating_load_is_in_band() {
    assert_strict(&strict_checks()[0]);
}

#[ignore = "the strict Annex B8 band selection: the out-of-band residuals it asserts \
     are the residual-attribution study's measured object and the study moves \
     no production number, so the tests stay red until a separately evidenced \
     physics row closes each residual; select with --run-ignored only"]
#[test]
fn strict_b8_case_395_annual_heating_load_is_in_band() {
    assert_strict(&strict_checks()[1]);
}

#[ignore = "the strict Annex B8 band selection: the out-of-band residuals it asserts \
     are the residual-attribution study's measured object and the study moves \
     no production number, so the tests stay red until a separately evidenced \
     physics row closes each residual; select with --run-ignored only"]
#[test]
fn strict_b8_case_900_annual_heating_load_is_in_band() {
    assert_strict(&strict_checks()[2]);
}

#[ignore = "the strict Annex B8 band selection: the out-of-band residuals it asserts \
     are the residual-attribution study's measured object and the study moves \
     no production number, so the tests stay red until a separately evidenced \
     physics row closes each residual; select with --run-ignored only"]
#[test]
fn strict_b8_case_600ff_peak_zone_temp_is_in_band() {
    assert_strict(&strict_checks()[3]);
}

#[ignore = "the strict Annex B8 band selection: the out-of-band residuals it asserts \
     are the residual-attribution study's measured object and the study moves \
     no production number, so the tests stay red until a separately evidenced \
     physics row closes each residual; select with --run-ignored only"]
#[test]
fn strict_b8_case_900ff_min_zone_temp_is_in_band() {
    assert_strict(&strict_checks()[4]);
}

/// The strict selection is exactly the five entries the header names: the
/// four out-of-band residuals plus Case 600's heating closure. A sixth
/// entry (Case 600's cooling, out of band at the parent on its own
/// recorded disposition) would silently grow the strict surface past the
/// dispatch's RED set; a missing entry would hide a residual.
#[test]
fn strict_selection_is_exactly_the_named_residual_set() {
    use BestestMetric::*;
    let checks = strict_checks();
    let named: Vec<(&str, BestestMetric)> = checks.iter().map(|c| (c.case_id, c.metric)).collect();
    assert_eq!(
        named,
        vec![
            ("600", AnnualHeatingLoadKwh),
            ("395", AnnualHeatingLoadKwh),
            ("900", AnnualHeatingLoadKwh),
            ("600FF", PeakZoneTempC),
            ("900FF", MinZoneTempC),
        ]
    );
    // Case 600's heating closure is the one green member: its band assert
    // ran green at the parent (4033.911883 in [3993, 4504]).
    let six_hundreds: Vec<&StrictCheck> = checks.iter().filter(|c| c.case_id == "600").collect();
    assert_eq!(
        six_hundreds.len(),
        1,
        "case 600 enters the strict selection once"
    );
    assert_eq!(six_hundreds[0].metric, AnnualHeatingLoadKwh);
}

/// The bands the strict selection asserts are the shared pinned rows: the
/// strict surface reads the same `core_reference_bands` the drift-lock
/// harness and the annual tests read, so the two surfaces can never
/// disagree about what the band is.
#[test]
fn strict_bands_are_the_shared_pinned_rows() {
    for check in strict_checks() {
        let band = band_for(check.case_id, check.metric);
        assert_eq!(
            band.case_id, check.case_id,
            "the pinned band names its own case"
        );
        assert!(band.max > band.min, "the pinned band is non-degenerate");
    }
}
