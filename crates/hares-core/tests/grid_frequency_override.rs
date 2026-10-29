//! The grid-frequency override's contract at the dwelling's setter.
//!
//! `Dwelling::set_frequency` is the frequency channel's input surface,
//! the companion of `set_grid_voltage`. Two defects closed here: the
//! frequency did not exist as an input at all, and `set_grid_voltage`
//! overwrote `frequency_hz` with the 60 Hz constant on every call. The
//! contract: the override is stored beside the voltage override and is
//! never overwritten by it; the 60 Hz constant is the fallback when no
//! override was given, never an overwrite of a set value; out-of-band
//! values are rejected with a typed error naming the value and the band,
//! leaving the stored grid state untouched.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use hares_core::Dwelling;
use hares_equipment::config::ConfigValue;
use hares_equipment::scheduled_load::ScheduledLoad;
use hares_equipment::{Equipment, EquipmentConfig};
use hares_types::EndUse;

fn write_minimal_toml(path: &Path) {
    let content = r#"building_id = 9402

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res = 60
duration = 1200
output_chunk_size = 1000
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.0

[schedule]
occupancy = 0.0
"#;
    fs::write(path, content).expect("failed to write synthetic TOML");
}

fn build_dwelling(tag: &str) -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let toml_path = dir.path().join(format!("{tag}.toml"));
    write_minimal_toml(&toml_path);
    let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

    let env = dwelling.latest_env().clone();
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    raw.insert("power_schedule_source".to_string(), "constant".into());
    raw.insert("power_constant_w".to_string(), 1500.0.into());
    raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
    raw.insert("zone_id".to_string(), 1.0.into());
    let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
    eq.init(&config, &env).expect("init ScheduledLoad");
    dwelling
        .add_equipment(Box::new(eq))
        .expect("add_equipment must succeed");
    dwelling
}

/// The pass-through witness: a set frequency reaches the simulation's
/// grid state at the next step, and it survives a later voltage call
/// (the overwrite defect's regression witness: the 60 Hz constant is the
/// fallback, never an overwrite).
#[test]
fn set_frequency_reaches_the_grid_state_and_survives_a_voltage_call() {
    let mut dwelling = build_dwelling("frequency-passthrough");

    dwelling.set_frequency(59.8).expect("an in-band frequency");
    dwelling
        .set_grid_voltage(0.7)
        .expect("a in-band sag voltage");
    dwelling.step().expect("the step applies the overrides");

    let grid = &dwelling.latest_env().grid;
    assert_eq!(grid.frequency_hz, 59.8, "the set frequency must survive");
    assert_eq!(grid.voltage_pu, 0.7, "the voltage override applies too");
}

/// A frequency-only run: the override composes with the default utility
/// voltage (the grid state the no-override path would have used).
#[test]
fn frequency_only_override_keeps_the_default_voltage() {
    let mut dwelling = build_dwelling("frequency-only");
    dwelling.set_frequency(59.3).expect("an in-band frequency");
    dwelling.step().expect("the step applies the override");
    let grid = &dwelling.latest_env().grid;
    assert_eq!(grid.frequency_hz, 59.3);
    assert_eq!(grid.voltage_pu, 1.0);
}

/// Without an override the 60 Hz constant falls back.
#[test]
fn without_an_override_the_60hz_constant_falls_back() {
    let mut dwelling = build_dwelling("frequency-default");
    dwelling.step().expect("a nominal step");
    assert_eq!(dwelling.latest_env().grid.frequency_hz, 60.0);
}

/// NaN, the infinities, zero, negatives, unit slips and out-of-band
/// values are rejected with a typed error naming the value and the band,
/// and the stored grid state is untouched by every rejection.
#[test]
fn out_of_contract_frequencies_are_rejected() {
    let mut dwelling = build_dwelling("frequency-reject");
    dwelling.set_frequency(59.8).expect("the control value");

    let before_grid = dwelling.latest_env().grid.clone();
    for invalid in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -59.8,
        39.9,
        80.1,
        600.0,
        0.0598,
    ] {
        let err = dwelling
            .set_frequency(invalid)
            .expect_err("an out-of-contract override must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("grid frequency override"),
            "the error names the override: {message}"
        );
        assert!(
            message.contains("[40, 80]"),
            "the error names the accepted band: {message}"
        );
        assert_eq!(
            dwelling.latest_env().grid,
            before_grid,
            "a rejected override must leave the stored grid state untouched"
        );
    }
}

/// The recorded non-model's contract witness (docs/PHYSICS_DECISIONS.md,
/// "The frequency channel is read by nothing"): the frequency channel is
/// accepted, validated and stored, and read by no load class: no
/// coefficient source exists in the toolchain (the OCHRE ZIP table's
/// columns are voltage-only; EnergyPlus supplies no supply-frequency
/// physics for zone or plant equipment). The brownout round's B4d probe
/// held a 59 Hz excursion beside a 0.7 pu sag and a pure 58 Hz excursion
/// and every state channel was bitwise identical. This test pins that
/// side-effect-freeness as the documented contract: when a frequency
/// response model lands, this assertion fails and forces the record's
/// retirement with it.
#[test]
fn in_band_frequency_excursions_change_no_state_channel() {
    let build = |tag: &str| {
        let mut dwelling = build_dwelling(tag);
        dwelling
            .set_grid_voltage(0.7)
            .expect("the sag the excursions ride beside");
        dwelling
    };
    let mut nominal = build("frequency-nominal");
    let mut f59 = build("frequency-59");
    let mut f58 = build("frequency-58");
    f59.set_frequency(59.0).expect("in-band");
    f58.set_frequency(58.0).expect("in-band");

    for _ in 0..8 {
        nominal.step().expect("step");
        f59.step().expect("step");
        f58.step().expect("step");
        // The stored frequencies reach the step (the channel's storage).
        assert_eq!(f59.latest_env().grid.frequency_hz, 59.0);
        assert_eq!(f58.latest_env().grid.frequency_hz, 58.0);
        let base = nominal.latest_env();
        for (tag, dw) in [("59 Hz", &f59), ("58 Hz", &f58)] {
            let other = dw.latest_env();
            assert_eq!(
                base.zones, other.zones,
                "{tag}: the zone channels must be bitwise identical (the recorded non-model: the frequency read by nothing)"
            );
            let base_core = base.equipment_core.get(&hares_types::EquipmentId(1));
            let other_core = other.equipment_core.get(&hares_types::EquipmentId(1));
            assert_eq!(
                base_core.map(|c| c.flows.electric_w),
                other_core.map(|c| c.flows.electric_w),
                "{tag}: the load's electric draw must be bitwise identical (the recorded non-model)"
            );
        }
    }
}
