use std::path::Path;

use hares_io::DefaultsStore;

fn defaults_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("defaults")
}

// CSV filenames like "Air Conditioner.csv" normalize to
// "biquadratic_air_conditioner". Tests use that full normalized key.

#[test]
fn loads_hvac_csv_cooling_curves() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let curves = store.hvac_cooling_curves("Air Conditioner");
    assert!(
        curves.is_some(),
        "Air Conditioner cooling curves should load from CSV"
    );
    let set = curves.unwrap();
    assert!(
        set.variants.len() >= 4,
        "CSV has 7 speed variants (Single, Double×2, Variable×4), got {}",
        set.variants.len()
    );
    assert_eq!(set.variants[0].name, "Single_1");
}

#[test]
fn loads_hvac_csv_heating_curves() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let curves = store.hvac_heating_curves("ASHP Heater");
    assert!(
        curves.is_some(),
        "ASHP Heater heating curves should load from CSV"
    );
    let set = curves.unwrap();
    assert!(!set.variants.is_empty());
    assert_eq!(set.variants[0].name, "Single_1");
}

#[test]
fn csv_cap_t_coefficients_match_known_values() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let curves = store
        .hvac_cooling_curves("Air Conditioner")
        .expect("AC curves");
    let single = &curves.variants[0];
    // From the CSV: a_cap_t = 1.5509 for Single_1
    assert!(
        (single.cap_t.coeffs[0] - 1.5509).abs() < 1e-4,
        "a_cap_t should be 1.5509, got {}",
        single.cap_t.coeffs[0]
    );
}

#[test]
fn csv_temperature_bounds_parsed() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let curves = store
        .hvac_cooling_curves("Air Conditioner")
        .expect("AC curves");
    let single = &curves.variants[0];
    assert!((single.cap_t.x1_bounds.0 - 13.88).abs() < 1e-2, "min_Twb");
    assert!((single.cap_t.x1_bounds.1 - 23.88).abs() < 1e-2, "max_Twb");
    assert!((single.cap_t.x2_bounds.0 - 18.33).abs() < 1e-2, "min_Tdb");
    assert!((single.cap_t.x2_bounds.1 - 51.66).abs() < 1e-2, "max_Tdb");
}

#[test]
fn csv_eir_plr_coefficients_parsed() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let curves = store
        .hvac_cooling_curves("Air Conditioner")
        .expect("AC curves");
    let single = &curves.variants[0];
    // a_eir_plr = 0.93, b_eir_plr = 0.07, c_eir_plr = 0 for Single_1
    assert!((single.eir_plr[0] - 0.93).abs() < 1e-4);
    assert!((single.eir_plr[1] - 0.07).abs() < 1e-4);
    assert!((single.eir_plr[2] - 0.0).abs() < 1e-4);
}

#[test]
fn mshp_cooler_has_variable_speed_variants() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let curves = store.hvac_cooling_curves("MSHP Cooler");
    assert!(curves.is_some(), "MSHP Cooler should load");
    let set = curves.unwrap();
    assert!(
        set.variants.iter().any(|v| v.name.starts_with("Variable")),
        "MSHP Cooler should have Variable speed variants"
    );
}

#[test]
fn envelope_lut_loaded_in_defaults_store() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    assert!(
        store.envelope_lut().is_some(),
        "envelope LUT should be loaded"
    );
}

#[test]
fn heating_ashp_eir_coefficients_differ_from_cooling() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let heat = store.hvac_heating_curves("ASHP Heater").expect("heating");
    let cool = store.hvac_cooling_curves("ASHP Cooler").expect("cooling");
    // They should have the same number of variants but different coefficients
    assert_eq!(heat.variants.len(), cool.variants.len());
    assert!(
        (heat.variants[0].eir_t.coeffs[0] - cool.variants[0].eir_t.coeffs[0]).abs() > 0.01,
        "heating and cooling EIR coefficients should differ"
    );
}

#[test]
fn air_conditioner_multispeed_2_speed_seer_17() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let params = store.hvac_multispeed_parameters("Air Conditioner", "SEER", 2, 17.0);
    assert!(
        params.is_some(),
        "Air Conditioner 2-speed SEER 17 should be found in CSV"
    );
    let p = params.unwrap();
    assert_eq!(p.number_of_speeds, 2);
    assert_eq!(p.capacity_ratios.len(), 2);
    assert!(
        (p.capacity_ratios[0] - 0.72).abs() < 1e-4,
        "first capacity ratio should be ~0.72"
    );
    assert!(
        (p.capacity_ratios[1] - 1.0).abs() < 1e-4,
        "last capacity ratio should be 1.0"
    );
    assert!(p.cops[0] > 3.0, "COP at stage 1 should be > 3");
    assert!(p.cops[1] > 3.0, "COP at stage 2 should be > 3");
}

#[test]
fn air_conditioner_multispeed_4_speed_seer_24() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let params = store.hvac_multispeed_parameters("Air Conditioner", "SEER", 4, 24.0);
    assert!(
        params.is_some(),
        "Air Conditioner 4-speed SEER 24 should be found in CSV"
    );
    let p = params.unwrap();
    assert_eq!(p.number_of_speeds, 4);
    assert_eq!(p.capacity_ratios.len(), 4);
    assert!(
        (p.capacity_ratios.last().copied().unwrap() - 1.0).abs() < 1e-4,
        "last capacity ratio should be 1.0"
    );
    assert!(!p.shrs.is_empty(), "SHRs should be populated");
}

#[test]
fn gshp_cooler_multispeed_2_speed_eer_20() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let params = store.hvac_multispeed_parameters("GSHP Cooler", "EER", 2, 20.0);
    assert!(
        params.is_some(),
        "GSHP Cooler 2-speed EER 20 should be found in CSV"
    );
    let p = params.unwrap();
    assert_eq!(p.number_of_speeds, 2);
    assert!(
        p.cops[0] > 5.0,
        "GSHP COP should be higher than ASHP due to ground source"
    );
}

#[test]
fn gshp_cooler_eer_kind_direct_lookup() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    // GSHP Cooler entries use "EER" as efficiency kind; verify direct
    // lookup by EER kind succeeds against the CSV entries.
    let params = store.hvac_multispeed_parameters("GSHP Cooler", "EER", 2, 18.0);
    assert!(
        params.is_some(),
        "GSHP Cooler should have entries searchable by EER kind"
    );
}

#[test]
fn gshp_heater_multispeed_2_speed_hspf_10() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let params = store.hvac_multispeed_parameters("GSHP Heater", "HSPF", 2, 10.0);
    assert!(
        params.is_some(),
        "GSHP Heater 2-speed HSPF 10 should be found in CSV"
    );
    let p = params.unwrap();
    assert_eq!(p.number_of_speeds, 2);
    assert!(p.cops[0] > 4.0, "GSHP heating COP should be > 4");
}

#[test]
fn gshp_heater_multispeed_4_speed_hspf_14() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    let params = store.hvac_multispeed_parameters("GSHP Heater", "HSPF", 4, 14.0);
    assert!(
        params.is_some(),
        "GSHP Heater 4-speed HSPF 14 should be found in CSV"
    );
    let p = params.unwrap();
    assert_eq!(p.number_of_speeds, 4);
    assert_eq!(p.capacity_ratios.len(), 4);
}

#[test]
fn air_conditioner_multispeed_closest_match_on_seer_distance() {
    let store = DefaultsStore::load(&defaults_dir()).expect("load defaults");
    // 17.5 SEER is between 17.0 and 18.0 entries, both equidistant;
    // min_by keeps the first match, which is 17.0.
    let params = store.hvac_multispeed_parameters("Air Conditioner", "SEER", 2, 17.5);
    assert!(params.is_some());
    let p = params.unwrap();
    // The closest SEER entry is 17.0 (first of two equidistant matches)
    assert!(
        (p.efficiency_value - 17.0).abs() < 0.1,
        "closest match to 17.5 SEER should be 17.0 (first equidistant), got efficiency_value={}",
        p.efficiency_value
    );
}
