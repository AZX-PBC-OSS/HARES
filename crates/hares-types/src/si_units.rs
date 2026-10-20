//! The two units `uom` 0.38 lacks and HARES's conversion factors need:
//! the `therm` (energy) and the `btu_it_per_hour` (power), defined with
//! `uom`'s exported `unit!` macro.
//!
//! The factors are the parent factors kept for bitwise neutrality:
//! - 1 therm = 100 000 of `uom`'s `btu_it` (105 505 600 J). `uom`'s
//!   `btu_it` is the rounded 1055.056 J, so this therm is 14.738 J larger
//!   than OpenStudio-HPXML v1.12.0's (1055.05585262 J per Btu times
//!   100 000); adopting that exact constant would be a numeric change of
//!   its own commit, not folded into the SI move.
//! - 1 btu_it_per_hour = 0.293_071_07 W (NIST), the value
//!   `BTU_PER_HOUR_TO_WATT` held in `hares-physics::units` before the SI
//!   move; `uom`'s own `btu_it_per_hour` differs by 5.9e-10 relative and
//!   is not the unit defined here.

pub mod energy {
    uom::unit! {
        system: uom::si;
        quantity: uom::si::energy;

        @therm: 1.055_056e8; "therm", "therm", "therms";
    }
}

pub mod power {
    uom::unit! {
        system: uom::si;
        quantity: uom::si::power;

        @btu_it_per_hour: 0.293_071_07; "Btu(IT)/h", "Btu(IT) per hour", "Btu(IT) per hour";
    }
}

#[cfg(test)]
mod acceptance {
    use super::energy::therm;
    use super::power::btu_it_per_hour;
    use uom::si::energy::{btu_it, joule};

    /// `therm_and_btu_per_hour_units_keep_the_parent_factors`:
    /// `Energy::new::<therm>(1.0).get::<joule>()` equals
    /// `Energy::new::<btu_it>(100_000.0).get::<joule>()` (the parent's
    /// definition, 105 505 600 J), within 1 ULP, and
    /// `Power::new::<btu_it_per_hour>(1.0).get::<watt>()` equals
    /// `0.29307107` on bits.
    #[test]
    fn therm_and_btu_per_hour_units_keep_the_parent_factors() {
        let one_therm_j = uom::si::f64::Energy::new::<therm>(1.0).get::<joule>();
        let hundred_k_btu_j = uom::si::f64::Energy::new::<btu_it>(100_000.0).get::<joule>();
        let ulp = f64::EPSILON * one_therm_j.abs().max(1.0);
        assert!(
            (one_therm_j - hundred_k_btu_j).abs() <= ulp,
            "the therm is 100 000 btu_it within 1 ULP: {one_therm_j} vs {hundred_k_btu_j}"
        );

        let one_btuh_w =
            uom::si::f64::Power::new::<btu_it_per_hour>(1.0).get::<uom::si::power::watt>();
        assert_eq!(
            one_btuh_w.to_bits(),
            0.293_071_07_f64.to_bits(),
            "one btu_it per hour is 0.29307107 W on bits, got {one_btuh_w}"
        );
    }
}
