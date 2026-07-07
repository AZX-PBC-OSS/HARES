//! Property-style invariants for the foundational CoreOutput types.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use hares_types::{
    ControlCapabilities, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance, CoreState,
    ElectricPower, EndUse, EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType,
    OperatingMode, Soc, validate_core_contract,
};

fn sample_unit(rng: &mut ChaCha8Rng) -> f64 {
    // Map to [0, 1) using the high 53 bits so the result is a stable finite f64.
    const DENOM: f64 = (1u64 << 53) as f64;
    let bits = rng.next_u64() >> 11;
    (bits as f64) / DENOM
}

fn sample_range(rng: &mut ChaCha8Rng, min: f64, max: f64) -> f64 {
    min + (max - min) * sample_unit(rng)
}

#[test]
fn electric_power_property_invariants_hold_for_random_finite_inputs() {
    let mut rng = ChaCha8Rng::seed_from_u64(0xC0FE_FEED_5EED_u64);

    for _ in 0..256 {
        let non_negative_kw = sample_range(&mut rng, 0.0, 1_000_000.0);
        let signed_kw = sample_range(&mut rng, -1_000_000.0, 1_000_000.0);
        let negative_kw = -sample_range(&mut rng, 0.000_001, 1_000_000.0);

        match ElectricPower::consumption(non_negative_kw).expect("valid consumption") {
            ElectricPower::Consumption(value) => assert_eq!(value, non_negative_kw),
            other => panic!("unexpected variant: {other:?}"),
        }

        match ElectricPower::generation(non_negative_kw).expect("valid generation") {
            ElectricPower::Generation(value) => assert_eq!(value, non_negative_kw),
            other => panic!("unexpected variant: {other:?}"),
        }

        match ElectricPower::bidirectional(signed_kw).expect("valid bidirectional") {
            ElectricPower::Bidirectional(value) => assert_eq!(value, signed_kw),
            other => panic!("unexpected variant: {other:?}"),
        }

        assert_eq!(
            ElectricPower::consumption(non_negative_kw)
                .expect("valid consumption")
                .net_consumption_kw(),
            non_negative_kw,
        );
        assert_eq!(
            ElectricPower::generation(non_negative_kw)
                .expect("valid generation")
                .net_consumption_kw(),
            -non_negative_kw,
        );
        assert_eq!(
            ElectricPower::bidirectional(signed_kw)
                .expect("valid bidirectional")
                .signed_kw(),
            signed_kw,
        );

        assert!(ElectricPower::consumption(negative_kw).is_err());
        assert!(ElectricPower::generation(negative_kw).is_err());
        assert!(ElectricPower::bidirectional(f64::INFINITY).is_err());
        assert!(ElectricPower::bidirectional(f64::NEG_INFINITY).is_err());
        assert!(ElectricPower::consumption(f64::NAN).is_err());
        assert!(ElectricPower::generation(f64::NAN).is_err());
    }
}

#[test]
fn soc_property_invariants_hold_for_random_finite_inputs() {
    let mut rng = ChaCha8Rng::seed_from_u64(0x5C0F_7EED_u64);

    for _ in 0..256 {
        let value = sample_range(&mut rng, -1.5, 2.5);
        let expected_valid = (0.0..=1.0).contains(&value);

        match Soc::try_from(value) {
            Ok(soc) => {
                assert!(
                    expected_valid,
                    "unexpected acceptance for out-of-range value {value}"
                );
                assert_eq!(soc.get(), value);
            }
            Err(_) => {
                assert!(
                    !expected_valid,
                    "unexpected rejection for in-range value {value}"
                );
            }
        }
    }

    assert!(Soc::try_from(0.0).is_ok());
    assert!(Soc::try_from(1.0).is_ok());
    assert!(Soc::try_from(-0.000_001).is_err());
    assert!(Soc::try_from(1.000_001).is_err());
}

#[test]
fn operating_mode_codes_are_stable_and_unique() {
    let modes = [
        OperatingMode::Off,
        OperatingMode::Heating,
        OperatingMode::Cooling,
        OperatingMode::Defrost,
        OperatingMode::Standby,
        OperatingMode::Charging,
        OperatingMode::Discharging,
        OperatingMode::HeatingHP,
        OperatingMode::HeatingER,
        OperatingMode::HeatingHPAndER,
        OperatingMode::HeatPumpWH,
        OperatingMode::BackupElement,
        OperatingMode::On,
    ];

    let mut seen_codes = Vec::with_capacity(modes.len());
    for (expected_code, mode) in modes.into_iter().enumerate() {
        let code_u8 = expected_code as u8;
        assert_eq!(mode as u8, code_u8);
        assert_eq!(mode.as_code(), code_u8 as f64);
        assert_eq!(
            OperatingMode::try_from(code_u8).expect("valid discriminant"),
            mode
        );
        seen_codes.push(mode.as_code());
    }

    for i in 0..seen_codes.len() {
        for j in (i + 1)..seen_codes.len() {
            assert_ne!(
                seen_codes[i], seen_codes[j],
                "OperatingMode codes must remain unique"
            );
        }
    }
}

fn full_descriptor(name: &str) -> EquipmentDescriptor {
    EquipmentDescriptor {
        id: EquipmentId(101),
        name: name.to_string(),
        end_use: EndUse::OTHER,
        equipment_type: "Test".into(),
        zone: None,
        fuel: FuelType::Electric,
        stage: ExecutionStage::Independent,
        control_capabilities: ControlCapabilities::empty(),
        core_capabilities: CoreCapabilities::THERMAL
            | CoreCapabilities::HAS_SETPOINT
            | CoreCapabilities::HAS_COP
            | CoreCapabilities::ELECTRIC
            | CoreCapabilities::REACTIVE,
        telemetry_fields: vec![],
        zone_type: None,
    }
}

#[test]
fn core_output_rejects_nan_in_any_bare_f64_field() {
    let desc = full_descriptor("NaN Rejector");

    let nan_fields = [
        (
            "flows.reactive_power_kvar",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(f64::NAN),
                    thermal_output_w: Some(1000.0),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(3.0),
                    ..Default::default()
                },
            },
        ),
        (
            "flows.thermal_output_w",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(0.0),
                    thermal_output_w: Some(f64::NAN),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(3.0),
                    ..Default::default()
                },
            },
        ),
        (
            "flows.sensible_cooling_w",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(0.0),
                    thermal_output_w: Some(1000.0),
                    sensible_cooling_w: Some(f64::NAN),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(3.0),
                    ..Default::default()
                },
            },
        ),
        (
            "flows.latent_cooling_w",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(0.0),
                    thermal_output_w: Some(1000.0),
                    latent_cooling_w: Some(f64::NAN),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(3.0),
                    ..Default::default()
                },
            },
        ),
        (
            "state.setpoint_c",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(0.0),
                    thermal_output_w: Some(1000.0),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(f64::NAN),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(3.0),
                    ..Default::default()
                },
            },
        ),
        (
            "performance.cop",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(0.0),
                    thermal_output_w: Some(1000.0),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(f64::NAN),
                    ..Default::default()
                },
            },
        ),
        (
            "performance.main_power_kw",
            CoreOutput {
                flows: CoreFlows {
                    electric_kw: Some(ElectricPower::Consumption(1.0)),
                    reactive_power_kvar: Some(0.0),
                    thermal_output_w: Some(1000.0),
                    ..Default::default()
                },
                state: CoreState {
                    setpoint_c: Some(20.0),
                    ..Default::default()
                },
                performance: CorePerformance {
                    cop: Some(3.0),
                    main_power_kw: Some(f64::NAN),
                },
            },
        ),
    ];

    for (field_name, out) in &nan_fields {
        let err = validate_core_contract(&desc, out)
            .expect_err(&format!("NaN in {field_name} must be rejected"));
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("non-finite value in {field_name}")),
            "unexpected error for {field_name}: {msg}"
        );
    }
}

#[test]
fn core_output_rejects_infinity_in_any_bare_f64_field() {
    let desc = full_descriptor("Inf Rejector");

    let non_finite_values = [f64::INFINITY, f64::NEG_INFINITY];
    let field_names = [
        "flows.reactive_power_kvar",
        "flows.thermal_output_w",
        "flows.sensible_cooling_w",
        "flows.latent_cooling_w",
        "state.setpoint_c",
        "performance.cop",
        "performance.main_power_kw",
    ];

    for &non_finite in &non_finite_values {
        for &field_name in &field_names {
            let out = match field_name {
                "flows.reactive_power_kvar" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(non_finite),
                        thermal_output_w: Some(1000.0),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(20.0),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(3.0),
                        ..Default::default()
                    },
                },
                "flows.thermal_output_w" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(0.0),
                        thermal_output_w: Some(non_finite),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(20.0),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(3.0),
                        ..Default::default()
                    },
                },
                "flows.sensible_cooling_w" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(0.0),
                        thermal_output_w: Some(1000.0),
                        sensible_cooling_w: Some(non_finite),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(20.0),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(3.0),
                        ..Default::default()
                    },
                },
                "flows.latent_cooling_w" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(0.0),
                        thermal_output_w: Some(1000.0),
                        latent_cooling_w: Some(non_finite),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(20.0),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(3.0),
                        ..Default::default()
                    },
                },
                "state.setpoint_c" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(0.0),
                        thermal_output_w: Some(1000.0),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(non_finite),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(3.0),
                        ..Default::default()
                    },
                },
                "performance.cop" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(0.0),
                        thermal_output_w: Some(1000.0),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(20.0),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(non_finite),
                        ..Default::default()
                    },
                },
                "performance.main_power_kw" => CoreOutput {
                    flows: CoreFlows {
                        electric_kw: Some(ElectricPower::Consumption(1.0)),
                        reactive_power_kvar: Some(0.0),
                        thermal_output_w: Some(1000.0),
                        ..Default::default()
                    },
                    state: CoreState {
                        setpoint_c: Some(20.0),
                        ..Default::default()
                    },
                    performance: CorePerformance {
                        cop: Some(3.0),
                        main_power_kw: Some(non_finite),
                    },
                },
                _ => unreachable!(),
            };

            let err = validate_core_contract(&desc, &out)
                .expect_err(&format!("{non_finite} in {field_name} must be rejected"));
            let msg = err.to_string();
            assert!(
                msg.contains(&format!("non-finite value in {field_name}")),
                "unexpected error for {field_name}={non_finite}: {msg}"
            );
        }
    }
}

#[test]
fn core_output_random_with_injected_nan_is_always_rejected() {
    let mut rng = ChaCha8Rng::seed_from_u64(0xDEAD_BEEF_5EED_u64);

    let desc = full_descriptor("Random NaN Probe");

    let bare_f64_slot_count = 7usize;
    let mut nan_used: [bool; 7] = [false; 7];

    for _ in 0..200 {
        let slot: usize = (rng.next_u64() as usize) % bare_f64_slot_count;

        let out = build_random_core_output_with_nan_in_slot(&mut rng, slot);
        nan_used[slot] = true;

        let err = validate_core_contract(&desc, &out)
            .expect_err(&format!("NaN in slot {slot} must be rejected"));
        let msg = err.to_string();
        assert!(
            msg.contains("non-finite value in"),
            "unexpected error for NaN in slot {slot}: {msg}"
        );
    }

    for (i, &used) in nan_used.iter().enumerate() {
        assert!(
            used,
            "Slot {i} was never selected for NaN injection; insufficient coverage"
        );
    }
}

fn build_random_core_output_with_nan_in_slot(rng: &mut ChaCha8Rng, slot: usize) -> CoreOutput {
    fn sample_finite(rng: &mut ChaCha8Rng) -> f64 {
        const DENOM: f64 = (1u64 << 53) as f64;
        let bits = rng.next_u64() >> 11;
        (bits as f64) / DENOM * 10_000.0
    }

    let (rpk, tow, scw, lcw, sp, cop_opt, mpk) = match slot {
        0 => (
            Some(f64::NAN),
            Some(sample_finite(rng)),
            None,
            None,
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
        ),
        1 => (
            Some(sample_finite(rng)),
            Some(f64::NAN),
            None,
            None,
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
        ),
        2 => (
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            Some(f64::NAN),
            None,
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
        ),
        3 => (
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
            Some(f64::NAN),
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
        ),
        4 => (
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
            None,
            Some(f64::NAN),
            Some(sample_finite(rng)),
            None,
        ),
        5 => (
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
            None,
            Some(sample_finite(rng)),
            Some(f64::NAN),
            None,
        ),
        6 => (
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            None,
            None,
            Some(sample_finite(rng)),
            Some(sample_finite(rng)),
            Some(f64::NAN),
        ),
        _ => unreachable!(),
    };

    CoreOutput {
        flows: CoreFlows {
            electric_kw: Some(ElectricPower::Consumption(1.0)),
            reactive_power_kvar: rpk,
            thermal_output_w: tow,
            sensible_cooling_w: scw,
            latent_cooling_w: lcw,
            ..Default::default()
        },
        state: CoreState {
            setpoint_c: sp,
            ..Default::default()
        },
        performance: CorePerformance {
            cop: cop_opt,
            main_power_kw: mpk,
        },
    }
}

#[test]
fn core_output_all_finite_is_accepted() {
    let desc = full_descriptor("Finite Producer");

    let out = CoreOutput {
        flows: CoreFlows {
            electric_kw: Some(ElectricPower::Consumption(1.0)),
            reactive_power_kvar: Some(0.1),
            thermal_output_w: Some(1000.0),
            sensible_cooling_w: Some(-500.0),
            latent_cooling_w: Some(-200.0),
            ..Default::default()
        },
        state: CoreState {
            setpoint_c: Some(20.0),
            ..Default::default()
        },
        performance: CorePerformance {
            cop: Some(3.5),
            main_power_kw: Some(2.1),
        },
    };
    validate_core_contract(&desc, &out).expect("all-finite CoreOutput must be accepted");
}
