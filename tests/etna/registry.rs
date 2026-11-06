//! The six-case contract registry: the enumeration rule, the per-case
//! data integrity, the example-software cross-checks, and the
//! Section 13 / Section 7 separation.

use super::kernel;
use super::measured::{self, case_contracts, example_software, measured_window};

/// The six ETNA twin-cell steady-state cases, in the addendum's
/// Table 13-1 order. The suite's conformance surface is ALL SIX: the
/// registry's enumeration test pins exactly this set, so a case whose
/// data is unpinnable stops the row rather than shipping a
/// fewer-than-six claim.
pub const SECTION_13_CASES: [&str; 6] = [
    "ET110A1", "ET110B1", "ET100A1", "ET100B1", "ET100A3", "ET100B3",
];

/// The registry carries exactly the six Section 13 cases, each with a
/// pinnable measured dataset: never fewer.
#[test]
fn the_registry_enumerates_exactly_the_six_section_13_cases() {
    let contracts = case_contracts();
    assert_eq!(
        contracts.len(),
        6,
        "the empirical suite's contract is all six Section 13 cases; a \
         case with unpinnable data stops the row instead of shipping a \
         fewer-than-six claim"
    );
    for (contract, expected) in contracts.iter().zip(SECTION_13_CASES.iter()) {
        assert_eq!(
            contract.id, *expected,
            "the registry's order is Table 13-1's"
        );
    }
    for id in SECTION_13_CASES {
        assert!(
            contracts.iter().any(|c| c.id == id),
            "case {id} is in the registry"
        );
    }
}

/// Each case's identity data matches the addendum's Table 13-1 row:
/// the cell, the window insulation, the interior-surface heat-transfer
/// mode, the base-case chain, and the specification/comparison section
/// references. The identity is what a case comparison's report names,
/// so a wrong identity would target the wrong construction.
#[test]
fn every_case_contract_carries_its_table_13_1_identity() {
    let expected: &[(&str, &str, bool, &str, &str, &str, &str)] = &[
        (
            "ET110A1",
            "A",
            true,
            "",
            "13.2.2",
            "13.3.2",
            "constant_combined",
        ),
        (
            "ET110B1",
            "B",
            true,
            "ET110A1",
            "13.2.3",
            "13.3.3",
            "constant_combined",
        ),
        (
            "ET100A1",
            "A",
            false,
            "ET110A1",
            "13.2.4",
            "13.3.4",
            "constant_combined",
        ),
        (
            "ET100B1",
            "B",
            false,
            "ET110B1",
            "13.2.5",
            "13.3.5",
            "constant_combined",
        ),
        (
            "ET100A3",
            "A",
            false,
            "ET100A1",
            "13.2.6",
            "13.3.6",
            "program_selected",
        ),
        (
            "ET100B3",
            "B",
            false,
            "ET100B1",
            "13.2.7",
            "13.3.7",
            "program_selected",
        ),
    ];
    for contract in case_contracts() {
        let row = expected
            .iter()
            .find(|(id, ..)| *id == contract.id)
            .unwrap_or_else(|| panic!("case {} is in Table 13-1", contract.id));
        let (
            _,
            cell,
            window_insulated,
            base_case,
            specification_section,
            comparison_section,
            interior_surface_ht,
        ) = *row;
        assert_eq!(contract.cell, cell, "case {}: the cell", contract.id);
        assert_eq!(
            contract.window_insulated, window_insulated,
            "case {}: the window insulation",
            contract.id
        );
        assert_eq!(
            contract.base_case, base_case,
            "case {}: the base-case chain",
            contract.id
        );
        assert_eq!(
            contract.specification_section, specification_section,
            "case {}: the specification section",
            contract.id
        );
        assert_eq!(
            contract.comparison_section, comparison_section,
            "case {}: the comparison section",
            contract.id
        );
        assert_eq!(
            contract.interior_surface_ht, interior_surface_ht,
            "case {}: the interior-surface heat-transfer mode",
            contract.id
        );
        // The contract's pinned measured bounds are the archive's
        // in-window summary values.
        let window =
            measured_window(contract).unwrap_or_else(|err| panic!("case {}: {err}", contract.id));
        assert_eq!(window.case_id, contract.id);
        assert!(
            (window.lower_wh - contract.qhtr_measured_lower_wh).abs() <= 1e-9,
            "case {}: the pinned lower bound {} is the archive's {}",
            contract.id,
            contract.qhtr_measured_lower_wh,
            window.lower_wh
        );
        assert!(
            (window.upper_wh - contract.qhtr_measured_upper_wh).abs() <= 1e-9,
            "case {}: the pinned upper bound {} is the archive's {}",
            contract.id,
            contract.qhtr_measured_upper_wh,
            window.upper_wh
        );
    }
}

/// Every case contract's window slices the archive to exactly the
/// window's hours, the in-window summary band is constant and centered
/// on the measured average, and the hourly mean equals the pinned
/// measured average.
#[test]
fn every_case_window_slices_the_archive_to_its_contract() {
    for contract in case_contracts() {
        let window =
            measured_window(contract).unwrap_or_else(|err| panic!("case {}: {err}", contract.id));
        assert_eq!(
            window.hourly_qhtr_wh.len(),
            contract.window_hours,
            "case {}: the sliced window's hour count is the contract's",
            contract.id
        );
        let hourly_mean =
            window.hourly_qhtr_wh.iter().sum::<f64>() / window.hourly_qhtr_wh.len() as f64;
        assert!(
            (hourly_mean - contract.qhtr_measured_avg_wh).abs() <= 1e-9,
            "case {}: the window's hourly mean {hourly_mean} equals the \
             pinned measured average {}",
            contract.id,
            contract.qhtr_measured_avg_wh
        );
        assert!(
            (window.avg_wh - contract.qhtr_measured_avg_wh).abs() <= 1e-9,
            "case {}: the archive's in-window summary average equals the \
             pinned value",
            contract.id
        );
        // The propagated band is centered on the measured average and
        // contains it.
        let center = (window.lower_wh + window.upper_wh) / 2.0;
        assert!(
            (center - window.avg_wh).abs() <= 1e-9,
            "case {}: the propagated band's center {} sits on the measured \
             average {}",
            contract.id,
            center,
            window.avg_wh
        );
        assert!(window.lower_wh <= window.avg_wh && window.avg_wh <= window.upper_wh);
        // The acceptance threshold's derivation: for the
        // constant-coefficient cases the threshold is the propagated
        // band fraction truncated to two decimals in percent; for the
        // program-selected-algorithm cases (the addendum's 13.2.6.1.2.a
        // substitution) the threshold is wider than the Qhtr band, the
        // room the program's own interior algorithms take.
        let band_rel = (window.upper_wh - window.avg_wh) / window.avg_wh;
        if contract.interior_surface_ht == "constant_combined" {
            assert!(
                contract.qhtr_acccrit_cvrmse_max <= band_rel
                    && band_rel < contract.qhtr_acccrit_cvrmse_max + 1e-4,
                "case {}: the acceptance threshold {} is the propagated band \
                 fraction {band_rel} truncated to two decimals in percent",
                contract.id,
                contract.qhtr_acccrit_cvrmse_max
            );
        } else {
            assert!(
                contract.interior_surface_ht == "program_selected"
                    && contract.qhtr_acccrit_cvrmse_max > band_rel,
                "case {}: the program-selected-algorithm threshold {} exceeds \
                 the propagated band fraction {band_rel}",
                contract.id,
                contract.qhtr_acccrit_cvrmse_max
            );
        }
        // The addendum's u(Qhtr) note value is narrower than or equal to
        // the propagated band: the propagated band contains the note's
        // band.
        assert!(
            contract.qhtr_uncertainty_rel <= band_rel,
            "case {}: the addendum note's u(Qhtr) {} cannot exceed the \
             propagated band fraction {band_rel}",
            contract.id,
            contract.qhtr_uncertainty_rel
        );
    }
}

/// The artificial climate holds the cell at 35 C and the guards at
/// approximately 10 C through the comparison window (the addendum's
/// 13.2.2.1.2 method; the guards' measured means are pinned per
/// dataset).
#[test]
fn the_artificial_climate_setpoints_hold_through_the_windows() {
    // The pinned in-window means [degC], recomputed from the committed
    // bytes on every run: a corrupted temperature series fails here
    // before any comparison runs.
    let pins: &[(&str, &[(&str, f64); 7])] = &[
        (
            "ET110A",
            &[
                ("Tattic", 9.77111111111111),
                ("Tcellar", 9.988888888888889),
                ("Tnorth", 10.257222222222222),
                ("Teast", 10.175),
                ("Tsouth", 9.90388888888889),
                ("Twest", 10.036666666666667),
                ("Tcell", 35.0),
            ],
        ),
        (
            "ET110B",
            &[
                ("Tattic", 10.030555555555555),
                ("Tcellar", 9.873333333333333),
                ("Tnorth", 10.005555555555555),
                ("Teast", 10.06388888888889),
                ("Tsouth", 10.105),
                ("Twest", 10.071111111111112),
                ("Tcell", 35.00388888888889),
            ],
        ),
        (
            "ET100A",
            &[
                ("Tattic", 10.671454545454546),
                ("Tcellar", 10.515818181818181),
                ("Tnorth", 10.352727272727272),
                ("Teast", 10.488727272727273),
                ("Tsouth", 10.41690909090909),
                ("Twest", 10.464727272727272),
                ("Tcell", 34.99963636363636),
            ],
        ),
        (
            "ET100B",
            &[
                ("Tattic", 10.420727272727273),
                ("Tcellar", 10.478181818181817),
                ("Tnorth", 10.364181818181818),
                ("Teast", 10.448),
                ("Tsouth", 10.608545454545455),
                ("Twest", 10.383454545454546),
                ("Tcell", 35.001090909090905),
            ],
        ),
    ];
    for (group, expected) in pins {
        let ds = measured::archive()
            .groups
            .get(*group)
            .unwrap_or_else(|| panic!("group {group} is loaded"));
        let (start, end) = match *group {
            "ET110A" | "ET110B" => ("2000-02-10 16:00:00", "2000-02-11 09:00:00"),
            _ => ("2000-09-16 08:00:00", "2000-09-18 14:00:00"),
        };
        let rows = ds
            .window(start, end)
            .unwrap_or_else(|err| panic!("group {group}: {err}"));
        for (column, pin) in expected.iter() {
            let idx = ds
                .column_index(column)
                .unwrap_or_else(|err| panic!("group {group}: {err}"));
            let mean = rows
                .iter()
                .map(|&r| {
                    ds.data[r][idx]
                        .unwrap_or_else(|| panic!("group {group}: null {column} in window"))
                })
                .sum::<f64>()
                / rows.len() as f64;
            assert!(
                (mean - pin).abs() <= 1e-9,
                "group {group}: the in-window mean of {column} is {mean}, \
                 the pinned measured value is {pin}"
            );
        }
        // The case's declared setpoints: the cell at 35 C, the guards
        // within 1 K of their 10 C setting (the addendum's
        // "approximately Tguards = 10 C").
        let cell_idx = ds
            .column_index("Tcell")
            .unwrap_or_else(|err| panic!("group {group}: {err}"));
        let cell_mean = rows
            .iter()
            .map(|&r| ds.data[r][cell_idx].expect("in-window cell temp"))
            .sum::<f64>()
            / rows.len() as f64;
        assert!(
            (cell_mean - 35.0).abs() <= 0.01,
            "group {group}: the test cell holds its 35 C setpoint, got \
             {cell_mean}"
        );
        for guard in ["Tattic", "Tcellar", "Tnorth", "Teast", "Tsouth", "Twest"] {
            let gidx = ds
                .column_index(guard)
                .unwrap_or_else(|err| panic!("group {group}: {err}"));
            let gmean = rows
                .iter()
                .map(|&r| ds.data[r][gidx].expect("in-window guard temp"))
                .sum::<f64>()
                / rows.len() as f64;
            assert!(
                (gmean - 10.0).abs() <= 1.0,
                "group {group}: the {guard} guard holds its approximately-10 C \
                 setting within 1 K through the window, got {gmean}"
            );
        }
    }
}

/// The kernel's CV(RMSE) values for the archive's own example programs,
/// computed independently with the archive's acceptance formula at the
/// extraction session and pinned here; every example program passes its
/// case's acceptance criterion (the addendum's B8.4.3.1: the example
/// results demonstrate the test specification's vetting). The
/// comparison derives nothing from HARES output: the software under
/// test here is the archive's own example data.
#[test]
fn the_kernel_reproduces_the_example_programs_independent_cv_values() {
    // (case, [(program, pinned CV(RMSE) as a fraction of the measured
    // average)]) computed with
    // sqrt(mean((sim_i - mean_meas)^2)) / mean_meas over the case's
    // best steady-state window, the archive's calc_cvrmse form.
    let pins: &[(&str, &[(&str, f64)])] = &[
        (
            "ET110A1",
            &[
                ("CSE", 0.003119607164223657),
                ("DeST", 0.0038938186263978605),
                ("DesignBuilder", 0.0021363073145586253),
                ("ESP-r", 0.00225326703387006),
                ("EnergyPlus", 0.0026303445489270455),
                ("IES-VE", 0.001685244759689478),
                ("TRNSYS", 0.0023980654318873037),
            ],
        ),
        (
            "ET110B1",
            &[
                ("CSE", 0.005840862906585284),
                ("DeST", 0.0083398540131439),
                ("DesignBuilder", 0.005731141429985429),
                ("ESP-r", 0.005376697116367792),
                ("EnergyPlus", 0.005111467067760034),
                ("TRNSYS", 0.004729507847259286),
            ],
        ),
        (
            "ET100A1",
            &[
                ("CSE", 0.0029657735149465317),
                ("DeST", 0.004191818642561533),
                ("DesignBuilder", 0.0031233332286792212),
                ("ESP-r", 0.0020562966691832405),
                ("EnergyPlus", 0.003187201154382272),
                ("IES-VE", 0.0020931351652018755),
                ("TRNSYS", 0.0033248070973545697),
            ],
        ),
        (
            "ET100B1",
            &[
                ("CSE", 0.003021928262413288),
                ("DeST", 0.005030315936584622),
                ("DesignBuilder", 0.003178006014930635),
                ("ESP-r", 0.0026553836801384),
                ("EnergyPlus", 0.003344970646657169),
                ("TRNSYS", 0.003884617709442443),
            ],
        ),
        (
            "ET100A3",
            &[
                ("CSE", 0.009617255487272868),
                ("DesignBuilder", 0.008143399994603706),
                ("ESP-r", 0.012548415289029221),
                ("EnergyPlus", 0.010577895206893237),
                ("IES-VE", 0.0035507187928256324),
                ("TRNSYS", 0.008289530499955029),
            ],
        ),
        (
            "ET100B3",
            &[
                ("CSE", 0.011469311593976574),
                ("DesignBuilder", 0.008617220440378633),
                ("ESP-r", 0.01933930745640499),
                ("EnergyPlus", 0.012481527158507217),
                ("TRNSYS", 0.008899938555195885),
            ],
        ),
    ];
    for (case_id, program_pins) in pins {
        let contract = case_contracts()
            .iter()
            .find(|c| &c.id == case_id)
            .unwrap_or_else(|| panic!("case {case_id} is in the registry"));
        let window =
            measured_window(contract).unwrap_or_else(|err| panic!("case {case_id}: {err}"));
        let case_entry = example_software()
            .cases
            .get(*case_id)
            .unwrap_or_else(|| panic!("case {case_id} is in the extracted example data"));
        let per_program = case_entry
            .get("qhtr_wh_per_program")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("case {case_id}: missing per-program Qhtr"));
        for (program, pin) in program_pins.iter() {
            let series: Vec<f64> = per_program
                .get(*program)
                .unwrap_or_else(|| panic!("{program} carries case {case_id}"))
                .as_array()
                .expect("the program's Qhtr array")
                .iter()
                .map(|v| v.as_f64().expect("finite Qhtr hour"))
                .collect();
            assert_eq!(
                series.len(),
                contract.window_hours,
                "{program} case {case_id}: the extracted window's hour count \
                 is the contract's"
            );
            let cv = kernel::cvrmse(case_id, &series, window.avg_wh)
                .unwrap_or_else(|err| panic!("{program} case {case_id}: {err}"));
            assert!(
                (cv - pin).abs() <= 1e-12,
                "{program} case {case_id}: the kernel's CV(RMSE) {cv} must \
                 reproduce the independently computed {pin}"
            );
            assert!(
                kernel::passes_acccrit(cv, contract.qhtr_acccrit_cvrmse_max),
                "{program} case {case_id}: the archive's example result \
                 passes the case's acceptance criterion (CV {cv} against \
                 threshold {})",
                contract.qhtr_acccrit_cvrmse_max
            );
        }
    }
}

/// The full comparison verdicts for every example program and case,
/// through the kernel's `compare_qhtr` surface. The archive's own
/// facts, pinned: all 37 program-case comparisons pass the CV(RMSE)
/// acceptance criterion, and 35 of 37 modeled averages sit within the
/// measured +/- u(Qhtr) band. The two exceptions are the archive's own
/// ESP-r results for the program-selected-algorithm cases (ET100A3
/// 745.665 Wh and ET100B3 751.722 Wh against the measured 736.615 /
/// 737.590 Wh with the 5.156 / 6.638 Wh half-widths): the addendum's
/// B8.4.3.1 anticipates them, "a tested program may disagree with
/// empirical data without necessarily being incorrect". The comparison
/// derives nothing from HARES output: the software under test here is
/// the archive's own example data.
#[test]
fn the_example_programs_compare_through_the_kernel() {
    let mut printed = Vec::new();
    let mut in_band = 0usize;
    let mut acc_pass = 0usize;
    let mut total = 0usize;
    for contract in case_contracts() {
        let window =
            measured_window(contract).unwrap_or_else(|err| panic!("case {}: {err}", contract.id));
        let case_entry = example_software()
            .cases
            .get(&contract.id)
            .unwrap_or_else(|| panic!("case {} is in the extracted example data", contract.id));
        let per_program = case_entry
            .get("qhtr_wh_per_program")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| panic!("case {}: missing per-program Qhtr", contract.id));
        for (program, series) in per_program {
            let series: Vec<f64> = series
                .as_array()
                .expect("the program's Qhtr array")
                .iter()
                .map(|v| v.as_f64().expect("finite Qhtr hour"))
                .collect();
            let comparison = kernel::compare_qhtr(
                &contract.id,
                &series,
                &window,
                contract.qhtr_uncertainty_rel,
                contract.qhtr_acccrit_cvrmse_max,
            )
            .unwrap_or_else(|err| panic!("{program} case {}: {err}", contract.id));
            // The kernel's band flag agrees with a direct recomputation
            // of the distance against the half-width.
            let distance = (comparison.modeled_avg_wh - comparison.measured_avg_wh).abs();
            assert_eq!(
                comparison.modeled_avg_in_band,
                distance <= comparison.band_half_width_wh,
                "{program} case {}: the band flag agrees with the distance",
                contract.id
            );
            // The archive's own evaluation: every example program
            // passes the CV(RMSE) acceptance criterion.
            assert!(
                comparison.acccrit_pass,
                "{program} case {}: the archive's example result passes the \
                 case's acceptance criterion (CV {} against threshold {})",
                contract.id, comparison.cvrmse, comparison.acccrit_max
            );
            if comparison.modeled_avg_in_band {
                in_band += 1;
            }
            acc_pass += 1;
            total += 1;
            printed.push(format!("{program}: {comparison}"));
        }
    }
    eprintln!("[etna] the archive's example results through the kernel:");
    for line in &printed {
        eprintln!("  {line}");
    }
    assert_eq!(
        total, 37,
        "the archive's example data carries 37 program-case comparisons \
         across the six cases (7+6+7+6+6+5: DeST stops at the A1/B1 cases, \
         IES-VE at the Cell A cases)"
    );
    assert_eq!(
        acc_pass, 37,
        "every example program passes its case's \
         CV(RMSE) acceptance criterion"
    );
    assert_eq!(
        in_band, 35,
        "35 of the 37 modeled averages sit within the measured +/- u(Qhtr) \
         band; the two exceptions are the archive's own ESP-r results for \
         the program-selected-algorithm cases"
    );
}

/// Section 13's identifiers stay out of the Section 7 core reference
/// bands: the annual BESTEST module's band registry is the Section 7
/// surface, and the empirical suite is a separate contract by
/// construction (the addendum's Annex B8.4 is additive; B8.1's Section 7
/// example values are unchanged).
#[test]
fn section_13_identifiers_stay_out_of_the_section_7_reference_bands() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/bestest/reference_bands.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    for identifier in ["ET110", "ET100", "ETNA", "Section 13"] {
        assert!(
            !source.contains(identifier),
            "the Section 7 core_reference_bands registry must not carry the \
             Section 13 identifier {identifier}: the empirical suite is a \
             separate contract, not a Section 7 band"
        );
    }
}

/// The empirical-validation target's files all live under tests/etna/:
/// the suite owns no shared file with the annual BESTEST module or any
/// production path (the wave's disjoint-ownership rule).
#[test]
fn the_empirical_target_shares_no_file_with_the_annual_module() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for file in ["mod.rs", "kernel.rs", "measured.rs", "registry.rs"] {
        let path = manifest.join("../../tests/etna").join(file);
        assert!(
            path.exists(),
            "the empirical target's {} must live under tests/etna/",
            file
        );
    }
    // The registration points at the target's own root, never into the
    // annual module's files.
    let cargo_toml = std::fs::read_to_string(manifest.join("Cargo.toml"))
        .expect("hares-core's Cargo.toml reads");
    let line = cargo_toml
        .lines()
        .find(|l| l.contains("tests/etna/mod.rs"))
        .expect("the etna test target is registered");
    assert!(
        line.contains("../../tests/etna/mod.rs"),
        "the etna target's registration points at tests/etna/mod.rs, got: {line}"
    );
}

/// The fixture directory's own hash pins are self-consistent: the
/// extraction's provenance names the unmodified upstream bundle, whose
/// sha256 the README records, and every hash string in the loader is
/// 64 hex characters (a truncated pin cannot silently pass).
#[test]
fn the_hash_pins_are_wellformed() {
    for pin in [
        measured::hashes::MEASUREMENT_DATA,
        measured::hashes::ET_CONFIG,
        measured::hashes::EXAMPLE_SOFTWARE_QHTR,
        measured::hashes::UPSTREAM_EXAMPLE_SOFTWARE_DATA,
        measured::hashes::SECTION13_ANALYSIS_FILES_ZIP,
        measured::hashes::ADDENDUM_PDF,
    ] {
        assert_eq!(pin.len(), 64, "the pin {pin} is a full sha256 hex string");
        assert!(
            pin.chars().all(|c| c.is_ascii_hexdigit()),
            "the pin {pin} is hex"
        );
    }
}
