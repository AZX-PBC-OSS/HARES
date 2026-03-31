//! Property-style invariants for the foundational CoreOutput types.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use hares_types::{ElectricPower, OperatingMode, Soc};

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
