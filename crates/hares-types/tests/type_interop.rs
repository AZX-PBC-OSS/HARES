//! Cross-layer type interoperability tests.
//!
//! These tests exercise the public API surface of hares-types as an external
//! crate consumer would see it -- re-exports, accumulation semantics, and
//! cross-type composition contracts that the inline unit tests do not cover
//! because they have access to private internals.
//!
//! Phase 1 gate: passes as soon as the core type PRs are done.

use chrono::{FixedOffset, TimeZone};
use hares_types::{
    ControlCapabilities, ControlSignal, EnvironmentState, GridState, PortContribution,
    PortDeclaration, PortSlots, ThermalAccumulator, ThermalCategory, WeatherState, ZoneId,
    ZoneState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn approx_eq(actual: f64, expected: f64, label: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{label}: actual={actual}, expected={expected}, delta={}",
        (actual - expected).abs()
    );
}

fn base_env() -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            outdoor_wet_bulb_c: 7.0,
            outdoor_enthalpy_j_kg: 22_800.0,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 180.0,
            mains_temp_c: 15.0,
            rainfall_m: 0.0,
            ground_albedo: 0.2,
            ground_t_mean_c: 10.0,
            ground_t_amplitude_c: 0.0,
            ground_phase_day: 35.0,
            day_of_year: 1.0,
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![],
        equipment_telemetry: std::collections::HashMap::new(),
        equipment_core: std::collections::HashMap::new(),
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: chrono::Duration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// PortSlots thermal accumulation via public PortDeclaration API
//
// The inline tests use the struct literal directly; these tests exercise the
// from_declarations factory, which is the path external crates must use.
// ---------------------------------------------------------------------------

/// Accumulating a Thermal contribution into a declared zone must update both
/// the total and the per-category subtotal atomically -- verifies the
/// cross-type contract between PortDeclaration, PortSlots, and ThermalAccumulator.
#[test]
fn thermal_contribution_accumulates_into_declared_zone() {
    let zone = ZoneId(0);
    let decls = [PortDeclaration::thermal(zone)];
    let mut slots = PortSlots::from_declarations(&decls);

    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 500.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 50.0,
            category: ThermalCategory::InternalGain,
        })
        .expect("accumulate must succeed for a declared zone");

    let acc = slots
        .thermal
        .iter()
        .find(|a| a.zone == zone)
        .expect("zone accumulator must be present after declaration");

    approx_eq(acc.sensible_gain_w, 500.0, "sensible total");
    approx_eq(acc.latent_gain_w, 50.0, "latent total");
    approx_eq(
        acc.sensible_for_category(ThermalCategory::InternalGain),
        500.0,
        "InternalGain category subtotal",
    );
    // Other categories must remain zero.
    approx_eq(
        acc.sensible_for_category(ThermalCategory::HvacHeating),
        0.0,
        "HvacHeating must be zero",
    );
}

/// Accumulating an Electrical contribution with positive active_power must
/// route to load, not generation. Negative active_power must route to generation.
#[test]
fn electrical_contribution_routes_load_vs_generation() {
    let mut slots = PortSlots::default();

    slots
        .accumulate(&PortContribution::Electrical {
            active_power_kw: 1.5,
            reactive_power_kvar: 0.0,
        })
        .expect("accumulate load must succeed");

    approx_eq(slots.electrical.load_power_kw, 1.5, "load kW");
    approx_eq(
        slots.electrical.generation_power_kw,
        0.0,
        "generation kW must be zero",
    );
    approx_eq(
        slots.electrical.net_active_kw(),
        1.5,
        "net must equal load when no generation",
    );

    slots
        .accumulate(&PortContribution::Electrical {
            active_power_kw: -1.0,
            reactive_power_kvar: 0.0,
        })
        .expect("accumulate generation must succeed");

    approx_eq(
        slots.electrical.load_power_kw,
        1.5,
        "load unchanged after adding generation",
    );
    approx_eq(slots.electrical.generation_power_kw, -1.0, "generation kW");
    approx_eq(
        slots.electrical.net_active_kw(),
        0.5,
        "net = load + generation",
    );
}

/// PortSlots::zero() must reset all accumulators to zero while preserving the
/// declared structure (thermal zone vec, fluid vec, etc.) for re-use.
#[test]
fn zero_resets_all_slots_while_preserving_structure() {
    let zone = ZoneId(1);
    let decls = [
        PortDeclaration::thermal(zone),
        PortDeclaration::electrical(),
        PortDeclaration::fuel(),
    ];
    let mut slots = PortSlots::from_declarations(&decls);

    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 300.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 30.0,
            category: ThermalCategory::HvacHeating,
        })
        .expect("accumulate thermal");
    slots
        .accumulate(&PortContribution::Electrical {
            active_power_kw: 2.0,
            reactive_power_kvar: 0.5,
        })
        .expect("accumulate electrical");

    // Pre-zero state must be non-zero.
    assert!(
        slots.thermal[0].sensible_gain_w > 0.0,
        "must have thermal gain before zero"
    );
    assert!(
        slots.electrical.load_power_kw > 0.0,
        "must have load before zero"
    );

    slots.zero();

    // Post-zero: all numeric fields must be exactly 0.
    let acc = slots
        .thermal
        .iter()
        .find(|a| a.zone == zone)
        .expect("zone must still exist");
    approx_eq(acc.sensible_gain_w, 0.0, "sensible after zero");
    approx_eq(acc.latent_gain_w, 0.0, "latent after zero");
    for cat in [
        ThermalCategory::HvacHeating,
        ThermalCategory::HvacCooling,
        ThermalCategory::InternalGain,
        ThermalCategory::JacketLoss,
        ThermalCategory::DuctLoss,
        ThermalCategory::HvacDehumidification,
    ] {
        approx_eq(
            acc.sensible_for_category(cat),
            0.0,
            "category subtotal after zero",
        );
    }
    approx_eq(slots.electrical.load_power_kw, 0.0, "load_power after zero");
    approx_eq(
        slots.electrical.generation_power_kw,
        0.0,
        "generation_power after zero",
    );
    approx_eq(
        slots.electrical.reactive_power_kvar,
        0.0,
        "reactive after zero",
    );
    approx_eq(slots.electrical.net_active_kw(), 0.0, "net after zero");

    // Structure is preserved: the thermal accumulator for the declared zone still exists.
    assert_eq!(
        slots.thermal.len(),
        1,
        "thermal slot count must survive zero"
    );
    assert_eq!(slots.thermal[0].zone, zone, "zone id must survive zero");
}

/// EnvironmentState constructed with all required fields must expose them
/// through the public type and custom_domains must be empty by default.
#[test]
fn environment_state_fields_accessible_and_custom_domains_empty() {
    let env = base_env();

    assert_eq!(env.zones.len(), 1, "expected one zone");
    assert_eq!(env.zones[0].id, ZoneId(1));
    assert!(
        (env.zones[0].temperature_c - 21.0).abs() < 1e-9,
        "zone temperature must be 21 C"
    );
    assert!(
        (env.weather.outdoor_temp_c - 10.0).abs() < 1e-9,
        "outdoor temp must be 10 C"
    );
    assert!(
        (env.grid.voltage_pu - 1.0).abs() < 1e-9,
        "grid voltage must be 1.0 pu"
    );
    assert!(
        env.custom_domains.is_empty(),
        "custom_domains must be empty by default"
    );
}

/// ControlSignal::ThermalSetpoint is accepted by a capability set that
/// includes THERMAL_SETPOINT and rejected by one that does not.
#[test]
fn control_signal_accepted_and_rejected_by_capability() {
    let signal = ControlSignal::ThermalSetpoint {
        heating_setpoint_c: Some(21.0),
        cooling_setpoint_c: Some(26.0),
        deadband_c: Some(1.0),
    };

    let matching_caps = ControlCapabilities::THERMAL_SETPOINT;
    let non_matching_caps = ControlCapabilities::POWER_SETPOINT;

    assert!(
        hares_types::ensure_signal_supported(matching_caps, &signal).is_ok(),
        "THERMAL_SETPOINT capability must accept ThermalSetpoint signal"
    );
    assert!(
        hares_types::ensure_signal_supported(non_matching_caps, &signal).is_err(),
        "POWER_SETPOINT capability must reject ThermalSetpoint signal"
    );
}

/// Accumulating to an undeclared zone must return an error, not silently drop.
/// This validates the cross-type boundary check in PortSlots::accumulate.
#[test]
fn accumulate_to_undeclared_zone_is_rejected() {
    let mut slots = PortSlots::from_declarations(&[PortDeclaration::thermal(ZoneId(1))]);
    let result = slots.accumulate(&PortContribution::Thermal {
        zone: ZoneId(42),
        sensible_gain_w: 100.0,
        radiant_gain_w: 0.0,
        latent_gain_w: 0.0,
        category: ThermalCategory::InternalGain,
    });
    assert!(
        result.is_err(),
        "accumulate to undeclared zone must return Err"
    );
}

/// ThermalAccumulator::new produces a zero-initialized accumulator bound to
/// the correct zone -- confirms the public constructor contract.
#[test]
fn thermal_accumulator_new_is_zero_and_bound_to_zone() {
    let zone = ZoneId(7);
    let acc = ThermalAccumulator::new(zone);
    assert_eq!(acc.zone, zone, "zone must match constructor argument");
    approx_eq(acc.sensible_gain_w, 0.0, "sensible must start at zero");
    approx_eq(acc.latent_gain_w, 0.0, "latent must start at zero");
    for cat in [
        ThermalCategory::HvacHeating,
        ThermalCategory::HvacCooling,
        ThermalCategory::InternalGain,
        ThermalCategory::JacketLoss,
        ThermalCategory::DuctLoss,
        ThermalCategory::HvacDehumidification,
    ] {
        approx_eq(
            acc.sensible_for_category(cat),
            0.0,
            "category subtotal must start at zero",
        );
    }
}

/// Two thermal contributions with different categories must accumulate their
/// per-category subtotals independently while updating the shared total.
/// This is the cross-type invariant: category routing + total consistency.
#[test]
fn mixed_category_totals_are_consistent() {
    let zone = ZoneId(2);
    let decls = [PortDeclaration::thermal(zone)];
    let mut slots = PortSlots::from_declarations(&decls);

    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 400.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 0.0,
            category: ThermalCategory::HvacHeating,
        })
        .expect("hvac heating accumulate");
    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 120.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 15.0,
            category: ThermalCategory::InternalGain,
        })
        .expect("internal gain accumulate");
    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 30.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 0.0,
            category: ThermalCategory::JacketLoss,
        })
        .expect("jacket loss accumulate");

    let acc = &slots.thermal[0];

    // Total must equal sum of all sensible contributions regardless of category.
    approx_eq(acc.sensible_gain_w, 550.0, "total sensible (400+120+30)");
    approx_eq(acc.latent_gain_w, 15.0, "total latent");

    // Per-category subtotals must match each contribution independently.
    approx_eq(
        acc.sensible_for_category(ThermalCategory::HvacHeating),
        400.0,
        "HvacHeating subtotal",
    );
    approx_eq(
        acc.sensible_for_category(ThermalCategory::InternalGain),
        120.0,
        "InternalGain subtotal",
    );
    approx_eq(
        acc.sensible_for_category(ThermalCategory::JacketLoss),
        30.0,
        "JacketLoss subtotal",
    );
    approx_eq(
        acc.sensible_for_category(ThermalCategory::HvacCooling),
        0.0,
        "HvacCooling must be zero",
    );

    // Invariant: per-category sum equals the total.
    let cat_sum = acc.sensible_for_category(ThermalCategory::HvacHeating)
        + acc.sensible_for_category(ThermalCategory::HvacCooling)
        + acc.sensible_for_category(ThermalCategory::InternalGain)
        + acc.sensible_for_category(ThermalCategory::JacketLoss)
        + acc.sensible_for_category(ThermalCategory::DuctLoss)
        + acc.sensible_for_category(ThermalCategory::HvacDehumidification);
    approx_eq(
        cat_sum,
        acc.sensible_gain_w,
        "sum of category subtotals must equal total sensible",
    );
}

// ---------------------------------------------------------------------------
// Regression tests: latent_by_category per-category breakdown
// ---------------------------------------------------------------------------

/// sum(latent_by_category) must equal latent_gain_w after a sequence of
/// mixed-category add() calls.
#[test]
fn latent_category_sum_matches_total() {
    let zone = ZoneId(3);
    let decls = [PortDeclaration::thermal(zone)];
    let mut slots = PortSlots::from_declarations(&decls);

    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 0.0,
            radiant_gain_w: 0.0,
            latent_gain_w: -500.0, // AC latent extraction
            category: ThermalCategory::HvacCooling,
        })
        .expect("hvac cooling latent");
    slots
        .accumulate(&PortContribution::Thermal {
            zone,
            sensible_gain_w: 0.0,
            radiant_gain_w: 0.0,
            latent_gain_w: 175.0, // occupant latent addition
            category: ThermalCategory::InternalGain,
        })
        .expect("occupant latent");

    let acc = &slots.thermal[0];

    // Aggregate total must reflect the net.
    approx_eq(acc.latent_gain_w, -325.0, "net latent (-500+175)");

    // Per-category subtotals must match each individual contribution.
    approx_eq(
        acc.latent_for_category(ThermalCategory::HvacCooling),
        -500.0,
        "HvacCooling latent subtotal",
    );
    approx_eq(
        acc.latent_for_category(ThermalCategory::InternalGain),
        175.0,
        "InternalGain latent subtotal",
    );
    approx_eq(
        acc.latent_for_category(ThermalCategory::HvacHeating),
        0.0,
        "HvacHeating latent must be zero",
    );

    // Invariant: sum of all per-category latent values equals latent_gain_w.
    let categories = [
        ThermalCategory::HvacHeating,
        ThermalCategory::HvacCooling,
        ThermalCategory::InternalGain,
        ThermalCategory::JacketLoss,
        ThermalCategory::DuctLoss,
        ThermalCategory::HvacDehumidification,
    ];
    let cat_sum: f64 = categories.iter().map(|&c| acc.latent_for_category(c)).sum();
    approx_eq(
        cat_sum,
        acc.latent_gain_w,
        "sum of latent category subtotals must equal latent_gain_w",
    );
}

/// zero() must reset latent_by_category -- every element must be 0.0.
#[test]
fn zero_resets_latent_by_category() {
    let zone = ZoneId(4);
    let mut acc = ThermalAccumulator::new(zone);

    acc.add(0.0, 0.0, 300.0, ThermalCategory::InternalGain);
    acc.add(0.0, 0.0, -200.0, ThermalCategory::HvacCooling);

    acc.zero();

    assert_eq!(
        acc.latent_gain_w, 0.0,
        "latent_gain_w must be zero after zero()"
    );

    let categories = [
        ThermalCategory::HvacHeating,
        ThermalCategory::HvacCooling,
        ThermalCategory::InternalGain,
        ThermalCategory::JacketLoss,
        ThermalCategory::DuctLoss,
        ThermalCategory::HvacDehumidification,
    ];
    for cat in categories {
        assert_eq!(
            acc.latent_for_category(cat),
            0.0,
            "latent_by_category[{cat:?}] must be 0.0 after zero()",
        );
    }
}
