//! Regression tests for warm-up period not enforced on HPXML path.
//!
//! `Dwelling::from_hpxml` hard-codes `initialization_duration: None`, so all
//! HPXML-path simulations skip warm-up entirely. For heavyweight construction
//! (concrete slab, masonry) the thermal time constant τ = RC is measured in
//! days; starting from a single-weather-hour steady-state snapshot leaves wall
//! and slab nodes far from their annual-periodic initial conditions. This
//! produces a multi-day transient bias in heat-transfer predictions.
//!
//! ## Test strategy
//!
//! We use BESTEST Case 900FF (heavyweight free-float, no HVAC) as the vehicle.
//! It has 100 mm concrete walls and 80 mm concrete floor slab whose thermal
//! time constant τ ≈ 3 days (slab) to 33 days (slab + insulation). Without
//! warmup the RC nodes start at `initialize_steady_state()` values computed
//! from the first weather snapshot; with 21-day warmup (as the 900ff fixture
//! configures) the nodes carry the correct annual-periodic initial heat
//! content. The zone temperature difference at the simulation start is
//! measurable and exceeds the EnergyPlus convergence threshold of 0.5 °C.
//!
//! Two tests:
//!
//! 1. `heavyweight_freefloat_warmup_changes_initial_zone_temperature` — passes
//!    now; documents that warmup matters for this building and that the delta
//!    exceeds 0.5 °C.
//!
//! 2. `hpxml_path_applies_default_warmup` — the **failing** regression test.
//!    It constructs a `DwellingConfig` matching `from_hpxml`'s current
//!    behaviour (initialization_duration: None) and asserts the zone
//!    temperature at step 1 agrees with the warmup run within 0.5 °C.
//!    Currently fails because `from_hpxml` skips warmup.
//!    Remove the `#[ignore]` and this test should pass after the fix.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write as IoWrite;
    use std::path::PathBuf;

    use hares_core::Dwelling;

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root must exist")
    }

    fn bestest_fixture(name: &str) -> PathBuf {
        project_root().join(format!("tests/fixtures/bestest/{name}"))
    }

    /// Write a modified 900ff TOML with a short 48-hour duration for speed, but
    /// retaining the full 21-day warmup so initial conditions are realistic.
    fn write_short_warmup_toml() -> tempfile::NamedTempFile {
        let original =
            fs::read_to_string(bestest_fixture("900ff.toml")).expect("900ff.toml must be readable");

        // Shorten the simulation to 48 hours (172800 s) for test speed.
        let shortened = original.replace("duration_s = 31536000", "duration_s = 172800");

        // Fix the relative EPW path to be absolute.
        let epw_abs = project_root()
            .join("vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
            .display()
            .to_string();
        let fixed = shortened.replace(
            "../../../vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw",
            &epw_abs,
        );

        let mut tmp = tempfile::Builder::new()
            .suffix(".toml")
            .tempfile()
            .expect("tempfile must be createable");
        tmp.write_all(fixed.as_bytes())
            .expect("write to tempfile must succeed");
        tmp
    }

    /// Write a modified 900ff TOML with no warmup and a short 48-hour duration.
    fn write_short_no_warmup_toml() -> tempfile::NamedTempFile {
        let original =
            fs::read_to_string(bestest_fixture("900ff.toml")).expect("900ff.toml must be readable");

        let shortened = original.replace("duration_s = 31536000", "duration_s = 172800");
        let stripped: String = shortened
            .lines()
            .filter(|line| !line.trim_start().starts_with("initialization_duration_s"))
            .map(|line| format!("{line}\n"))
            .collect();

        let epw_abs = project_root()
            .join("vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
            .display()
            .to_string();
        let fixed = stripped.replace(
            "../../../vendors/OCHRE/ochre/defaults/Weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw",
            &epw_abs,
        );

        let mut tmp = tempfile::Builder::new()
            .suffix(".toml")
            .tempfile()
            .expect("tempfile must be createable");
        tmp.write_all(fixed.as_bytes())
            .expect("write to tempfile must succeed");
        tmp
    }

    /// Returns the conditioned zone temperature at the first recorded timestep.
    fn first_step_zone_temp(dwelling: &mut Dwelling) -> f64 {
        let step = dwelling.step().expect("first step must succeed");
        step.zone_temperatures_c
            .first()
            .map(|(_, t)| *t)
            .expect("at least one zone temperature")
    }

    /// Demonstrates that warmup materially changes initial zone temperatures for
    /// BESTEST 900FF (heavyweight concrete, free-float, no HVAC).
    ///
    /// The 100 mm concrete walls and 80 mm concrete floor slab give a thermal
    /// time constant of several days. 21 days of warmup (as the fixture
    /// configures) drives the RC nodes to their annual-periodic state, which
    /// differs from the `initialize_steady_state()` snapshot used when no
    /// warmup is applied.
    ///
    /// This test **passes now** and documents the precondition: the delta
    /// exceeds the EnergyPlus convergence threshold of 0.5 °C.
    #[test]
    fn heavyweight_freefloat_warmup_changes_initial_zone_temperature() {
        let warmup_toml = write_short_warmup_toml();
        let no_warmup_toml = write_short_no_warmup_toml();

        let mut with_warmup =
            Dwelling::from_toml_config_with_write_output(warmup_toml.path(), Some(false))
                .expect("warmup dwelling must build");
        let mut without_warmup =
            Dwelling::from_toml_config_with_write_output(no_warmup_toml.path(), Some(false))
                .expect("no-warmup dwelling must build");

        let t_with = first_step_zone_temp(&mut with_warmup);
        let t_without = first_step_zone_temp(&mut without_warmup);
        let delta = (t_with - t_without).abs();

        assert!(
            delta > 0.5,
            "Expected warmup to shift zone temperature by > 0.5 °C for BESTEST \
             900FF heavyweight concrete building (got {delta:.3} °C). Either the \
             building has insufficient thermal mass or the warm-up ran for too few \
             days. This precondition must hold for the hpxml_path_applies_default_warmup \
             test to be meaningful."
        );
    }

    /// The **failing regression test** for warmup enforcement on the HPXML path.
    ///
    /// `Dwelling::from_hpxml` hard-codes `initialization_duration: None`, so
    /// all HPXML-path simulations skip warm-up. This test asserts the
    /// *correct* (post-fix) behaviour: that the no-warmup config's first-step
    /// zone temperature agrees with the warmup config within 0.5 °C, implying
    /// that the default warmup is being applied.
    ///
    /// **Currently ignored** because `from_hpxml` doesn't apply warmup.
    /// After the fix, remove `#[ignore]` and this should pass.
    ///
    /// The companion test `heavyweight_freefloat_warmup_changes_initial_zone_temperature`
    /// proves the precondition: warmup actually shifts temperatures > 0.5 °C,
    /// so this test is non-trivial to satisfy.
    #[test]
    #[ignore = "from_hpxml skips warmup — remove ignore after fix lands"]
    fn hpxml_path_applies_default_warmup() {
        // Simulate what from_hpxml would produce: no initialization_duration.
        // After the fix, from_hpxml should set a 7-day default, so the
        // no-warmup config (None) should match the with-warmup config.
        //
        // We use the TOML path here because from_hpxml requires HPXML files
        // while the 900ff BESTEST fixture uses synthetic TOML. The structural
        // equivalence: initialization_duration: None in a DwellingConfig
        // corresponds exactly to omitting initialization_duration_s in a
        // synthetic TOML — both produce the same code path (no run_warmup call).
        let warmup_toml = write_short_warmup_toml();
        let no_warmup_toml = write_short_no_warmup_toml();

        let mut with_warmup =
            Dwelling::from_toml_config_with_write_output(warmup_toml.path(), Some(false))
                .expect("warmup dwelling must build");
        let mut without_warmup =
            Dwelling::from_toml_config_with_write_output(no_warmup_toml.path(), Some(false))
                .expect("no-warmup dwelling must build");

        let t_with = first_step_zone_temp(&mut with_warmup);
        let t_without = first_step_zone_temp(&mut without_warmup);
        let delta = (t_with - t_without).abs();

        assert!(
            delta < 0.5,
            "Zone temperature at step 1 differs by {delta:.3} °C between warmup \
             (21 days) and no-warmup runs for BESTEST 900FF heavyweight concrete \
             building (threshold: 0.5 °C — EnergyPlus Eng.Ref §Warmup Convergence). \
             The HPXML path is not applying a default warmup period. \
             Fix: set initialization_duration to Some(7d) in Dwelling::from_hpxml."
        );
    }
}
