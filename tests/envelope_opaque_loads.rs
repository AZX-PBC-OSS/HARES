//! Zone-load reporting contract for the opaque envelope: any quantity
//! presented to consumers as the opaque envelope's heat gain *to the indoor
//! zone* must be the net conduction delivered to that zone, and the zone's
//! net sensible balance must never include the gross radiant flux absorbed
//! at the exterior skin.
//!
//! The gross exterior solar + longwave absorption
//! (`EnvelopeComponentGains::opaque_solar_lwr_w`) is a boundary condition on
//! each opaque surface's own exterior RC node — the same quantity OCHRE
//! reports as "{boundary} Ext. Solar/LWR Gain". Most of it re-leaves via
//! exterior convection and sky longwave exchange; only a small, time-lagged
//! fraction conducts through the wall/roof/floor into the zone. HARES
//! computes that net fraction per timestep (`wall_heat_gain_w` /
//! `floor_heat_gain_w` / `roof_heat_gain_w`), exposes it as the
//! "Wall/Floor/Roof Heat Gain - Indoor (W)" columns, and aggregates it into
//! "Opaque Surface Heat Gain - Indoor (W)" and
//! `EnvelopeComponentLoadsKwh::opaque_conduction_kwh`.
//!
//! Both assertions compare reported output columns against each other, so
//! they are presentation-independent: wiring any exterior boundary flux back
//! into an "Indoor" heat-gain presentation fails here, as does inflating the
//! net sensible balance with it (~13.5 kW mean gross vs ~100 W net on this
//! fixture's summer day).

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use chrono::{Duration, FixedOffset, TimeZone};
    use hares_core::{DwellingConfig, SimulationConfig, SimulationEngine};
    use hares_io::{EnvelopeComponentLoadsKwh, OutputFormat};

    fn unique_temp_name(base: &str, ext: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        format!("{base}_{nanos}.{ext}")
    }

    fn examples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/examples")
    }

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Parse a CSV into a column-name -> Vec<f64> map.
    /// Non-numeric cells (timestamps) are silently skipped per-cell.
    fn parse_csv_columns(path: &PathBuf) -> BTreeMap<String, Vec<f64>> {
        let contents = fs::read_to_string(path).expect("read CSV");
        let mut lines = contents.lines();
        let header = lines.next().expect("header line");
        let columns: Vec<String> = header.split(',').map(|c| c.trim().to_string()).collect();
        let mut data: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for col in &columns {
            data.insert(col.clone(), Vec::new());
        }
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            for (i, field) in fields.iter().enumerate() {
                if i < columns.len() {
                    if let Ok(v) = field.trim().parse::<f64>() {
                        data.get_mut(&columns[i]).unwrap().push(v);
                    }
                }
            }
        }
        data
    }

    fn column_values<'a>(data: &'a BTreeMap<String, Vec<f64>>, name: &str) -> &'a [f64] {
        let values = data
            .get(name)
            .unwrap_or_else(|| panic!("expected column '{name}' in output CSV"));
        assert!(
            !values.is_empty(),
            "column '{name}' has no numeric values -- check would be vacuous"
        );
        values
    }

    fn column_mean(data: &BTreeMap<String, Vec<f64>>, name: &str) -> f64 {
        let values = column_values(data, name);
        values.iter().sum::<f64>() / values.len() as f64
    }

    /// One shared 24 h summer simulation (BEopt example house, Denver TMY3,
    /// 1-minute resolution, verbosity 6). Strong exterior solar absorption
    /// on roof/walls is what separates the gross exterior flux from the net
    /// conducted load, so a summer day is the discriminating fixture. Returns
    /// the parsed output CSV columns plus the run-level envelope load metrics
    /// (the streaming recorder path, `retain_batches = false`).
    fn summer_24h_run() -> &'static (BTreeMap<String, Vec<f64>>, EnvelopeComponentLoadsKwh) {
        static RUN: OnceLock<(BTreeMap<String, Vec<f64>>, EnvelopeComponentLoadsKwh)> =
            OnceLock::new();
        RUN.get_or_init(|| {
            let output_path =
                std::env::temp_dir().join(unique_temp_name("hares_opaque_loads_summer_24h", "csv"));
            let denver = FixedOffset::west_opt(7 * 3600).expect("Denver UTC-7 offset");
            let config = DwellingConfig {
                hpxml_path: examples_dir().join("BEopt_example.xml"),
                schedule_path: examples_dir().join("BEopt_example_schedule.csv"),
                weather_path: examples_dir().join("USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
                defaults_path: Some(project_root().join("defaults")),
                sim_config: SimulationConfig {
                    start_time: denver.with_ymd_and_hms(2019, 7, 15, 0, 0, 0).unwrap(),
                    duration: Duration::hours(24),
                    time_res: Duration::minutes(1),
                    output_verbosity: 6,
                    output_path: Some(output_path.clone()),
                    write_output: true,
                    output_format: OutputFormat::Csv,
                    output_chunk_size: 1024,
                    setpoint_deadband_c: None,
                    master_seed: 42,
                    civil_timezone: None,
                    site_location: hares_io::SiteLocationOverride::default(),
                    retain_batches: false,
                    rotation: hares_io::RotationPolicy::None,
                },
                overrides: None,
                bldg_id: 1,
                initialization_duration: None,
                resample_overrides: Some(hares_io::ResampleOverrides::ochre_compat()),
                patches: None,
            };

            let engine = SimulationEngine::new();
            let result = engine.run(config).expect("engine.run should succeed");
            let envelope_loads = result
                .metrics
                .envelope_loads_kwh
                .expect("verbosity 6 must produce envelope-load metrics");

            let data = parse_csv_columns(&output_path);
            let _ = fs::remove_file(&output_path);
            (data, envelope_loads)
        })
    }

    fn summer_24h_columns() -> &'static BTreeMap<String, Vec<f64>> {
        &summer_24h_run().0
    }

    /// A quantity reported as the opaque envelope's heat gain to the indoor
    /// zone must equal the net conduction from the opaque boundaries
    /// (wall + floor + roof interior surfaces) into that zone.
    #[test]
    fn opaque_surface_indoor_heat_gain_is_net_conduction() {
        let data = summer_24h_columns();

        let reported_opaque_w = column_mean(data, "Opaque Surface Heat Gain - Indoor (W)");
        let net_opaque_conduction_w = column_mean(data, "Wall Heat Gain - Indoor (W)")
            + column_mean(data, "Floor Heat Gain - Indoor (W)")
            + column_mean(data, "Roof Heat Gain - Indoor (W)");

        eprintln!("  reported 'Opaque Surface Heat Gain - Indoor' mean: {reported_opaque_w:.1} W");
        eprintln!(
            "  net opaque conduction (Wall+Floor+Roof Heat Gain - Indoor) mean: \
             {net_opaque_conduction_w:.1} W"
        );

        // Guard against silent-zero regressions: a mean-vs-mean comparison is
        // vacuous when both sides are zero, and release builds once emitted
        // 0 W here when the per-boundary convection accumulator was
        // debug-gated. Over a full Denver summer day the envelope conducts a
        // nonzero flux in at least one timestep in any build configuration.
        assert!(
            column_values(data, "Opaque Surface Heat Gain - Indoor (W)")
                .iter()
                .any(|v| v.abs() > 1e-9),
            "'Opaque Surface Heat Gain - Indoor (W)' is all zeros: the net \
             conduction accumulator is not running in this build configuration"
        );

        // The tolerance is generous in absolute terms: it absorbs boundary
        // categorization detail (e.g. opaque surfaces adjacent to
        // non-conditioned zones) while remaining far tighter than the
        // gross-vs-net gap (~13 kW gross vs tens of watts net on this
        // fixture). What it must not absorb is wiring the gross exterior
        // absorption into a field presented as a zone load.
        let tolerance_w = 200.0_f64.max(0.5 * net_opaque_conduction_w.abs());
        assert!(
            (reported_opaque_w - net_opaque_conduction_w).abs() <= tolerance_w,
            "'Opaque Surface Heat Gain - Indoor (W)' mean {reported_opaque_w:.1} W disagrees \
             with net opaque conduction into the zone {net_opaque_conduction_w:.1} W by \
             {:.1} W (tolerance {tolerance_w:.1} W): the reported opaque term is the gross \
             exterior solar+LWR absorption, not the net heat delivered to the zone, so it \
             cannot be summed into the zone's envelope load budget as documented",
            (reported_opaque_w - net_opaque_conduction_w).abs()
        );
    }

    /// The zone's net sensible balance must be the sum of the direct heat
    /// injections on zone air (OCHRE's "Net Sensible Heat Gain - {zone} (W)"
    /// semantics) — it must not include the gross exterior solar+LWR
    /// absorption, which is a boundary condition on exterior RC nodes, not a
    /// zone input.
    #[test]
    fn net_sensible_excludes_gross_exterior_absorption() {
        let data = summer_24h_columns();

        let net_sensible_w = column_mean(data, "Net Sensible Heat Gain - Indoor (W)");
        // Direct injections on zone air, each with its own CSV column. HVAC
        // cooling is emitted as a positive magnitude but enters the net
        // balance as a negative gain. Equipment jacket loss has no CSV
        // column; the tolerance absorbs it.
        let direct_injection_w = column_mean(data, "Window Transmitted Solar Gain (W)")
            + column_mean(data, "Infiltration Heat Gain - Indoor (W)")
            + column_mean(data, "Forced Ventilation Heat Gain - Indoor (W)")
            + column_mean(data, "Natural Ventilation Heat Gain - Indoor (W)")
            + column_mean(data, "Internal Heat Gain - Indoor (W)")
            + column_mean(data, "Duct Loss Heat Gain - Indoor (W)")
            + column_mean(data, "HVAC Heating Delivered (W)")
            - column_mean(data, "HVAC Cooling Delivered (W)");

        eprintln!("  net sensible mean: {net_sensible_w:.1} W");
        eprintln!("  direct injection mean (excl. jacket loss): {direct_injection_w:.1} W");

        // Absorbs the residual vs the column-sum, which equals the equipment
        // jacket loss — the only direct-injection term without a CSV column
        // (~43 W mean observed; this fixture's water heater sits in the living
        // space, BEopt_example.xml) — while staying two orders of magnitude
        // below the gross exterior absorption (~13.5 kW mean on this fixture),
        // which is the inflation this test guards against.
        let tolerance_w = 250.0;
        assert!(
            (net_sensible_w - direct_injection_w).abs() <= tolerance_w,
            "'Net Sensible Heat Gain - Indoor (W)' mean {net_sensible_w:.1} W disagrees with \
             the direct zone-air injections {direct_injection_w:.1} W by {:.1} W (tolerance \
             {tolerance_w:.1} W): the net sensible balance must not include the gross \
             exterior solar+LWR absorption or opaque conduction (both flow through the \
             envelope RC network, not the zone heat input)",
            (net_sensible_w - direct_injection_w).abs()
        );
    }

    /// The zone air heat-balance residual column must exist and stay
    /// small against the loads it reconciles. The residual is NOT expected
    /// to be ~0 (semi-implicit coupling and one-step state lags leave a
    /// physical remainder); what it must never do is reach the magnitude of
    /// the loads themselves — that signature means a gain is mis-wired or a
    /// boundary-condition flux is leaking into zone-load terms (I-02 class).
    #[test]
    fn zone_air_balance_residual_is_small_against_loads() {
        let data = summer_24h_columns();
        let residual = column_values(data, "Zone Air Heat Balance Residual (W)");
        let max_abs = residual.iter().map(|v| v.abs()).fold(0.0, f64::max);
        let mean_abs = residual.iter().map(|v| v.abs()).sum::<f64>() / residual.len() as f64;
        let net = column_values(data, "Net Sensible Heat Gain - Indoor (W)");
        let net_max = net.iter().map(|v| v.abs()).fold(0.0, f64::max);
        eprintln!(
            "[residual] zone air balance residual: max |r| = {max_abs:.2} W, \
             mean |r| = {mean_abs:.2} W, max |net sensible| = {net_max:.2} W"
        );
        // Bound rationale: this fixture cycles HVAC; the thermostat acts on
        // the committed state while capacity lands in the same step, so
        // on/off transitions carry an inherent one-step mismatch of
        // O(capacity·dt fraction). Observed max 1.18 kW (mean 0.34 kW)
        // against 16.4 kW peak loads. The bound 2.5 kW is ~2× observed max:
        // tolerates cycling transients, still catches the I-02 class
        // (a gross-flux leak would be O(10 kW) on this fixture).
        // The tight bound lives on the freefloat case (no HVAC timing) — see
        // `bestest::case_600ff_zone_air_balance_residual_bounded`.
        assert!(
            max_abs < 2500.0,
            "zone air heat-balance residual reached {max_abs:.1} W — O(kW-class \
             residual means a mis-wired gain or a boundary-condition flux in \
             zone-load terms (I-02 class)"
        );
    }

    /// No-silent-zeros meta-test: every
    /// heat-balance diagnostic column must exist AND be non-degenerate — at
    /// least one |value| > 1 mW over the day. This generalizes the guard for
    /// the cfg-gating defect class (diagnostics compiled out in release
    /// silently reporting 0 W): a column that exists but is identically zero
    /// on a Denver summer day with strong solar, infiltration, and internal
    /// gains is a broken accumulator, whatever the build mode.
    #[test]
    fn heat_balance_columns_exist_and_are_nondegenerate() {
        let data = summer_24h_columns();
        const COLUMNS: &[&str] = &[
            "Net Sensible Heat Gain - Indoor (W)",
            "Internal Heat Gain - Indoor (W)",
            "Opaque Surface Heat Gain - Indoor (W)",
            "Roof Heat Gain - Indoor (W)",
            "Floor Heat Gain - Indoor (W)",
            "Wall Heat Gain - Indoor (W)",
            "Window Heat Gain - Indoor (W)",
            "Internal Mass Heat Gain - Indoor (W)",
            "Infiltration Heat Gain - Indoor (W)",
            "Window Transmitted Solar Gain (W)",
        ];
        for &name in COLUMNS {
            let values = column_values(data, name); // panics if missing
            assert!(
                values.iter().all(|v| v.is_finite()),
                "column '{name}' contains non-finite values"
            );
            let max_abs = values.iter().map(|v| v.abs()).fold(0.0, f64::max);
            assert!(
                max_abs > 1e-3,
                "column '{name}' is degenerate (max |value| = {max_abs:.3e} W over the \
                 day): a heat-balance diagnostic that is identically ~0 on a summer-day \
                 run is a silently-broken accumulator (cfg-gating class)"
            );
        }
    }

    /// The user-facing contract of `EnvelopeComponentLoadsKwh`: every field
    /// except `interior_lwr_kwh` (the documented gross-exchange exception) is
    /// a net, signed zone load, and together they close the zone air heat
    /// balance. For a zone held near its setpoint over a full day, the signed
    /// sum of net gains must nearly cancel (what remains is the small air
    /// storage change plus the semi-implicit coupling residual pinned by
    /// `zone_air_balance_residual_is_small_against_loads`), so
    /// |sum| must be a small fraction of sum|component|.
    ///
    /// This is the discriminating signature of the gross-vs-net defect class
    /// at the metrics level: a gross exterior flux is positive-dominant
    /// (solar absorption), so polluting any budget field with it pushes the
    /// signed sum toward sum|component| (the ticket's +141 MWh/yr sum, ~8x
    /// the home's consumption, is a ratio near 1). Net physical gains and
    /// losses largely cancel in a controlled zone, giving a ratio far below
    /// the bound.
    #[test]
    fn envelope_load_budget_is_signed_summable_and_closes() {
        let env = &summer_24h_run().1;

        // Cooling is stored as a positive delivered magnitude; it enters the
        // zone balance as a negative gain (same convention as the
        // net-sensible column check above).
        let components: [(&str, f64); 10] = [
            ("window_solar_kwh", env.window_solar_kwh),
            ("window_conduction_kwh", env.window_conduction_kwh),
            ("opaque_conduction_kwh", env.opaque_conduction_kwh),
            ("infiltration_kwh", env.infiltration_kwh),
            ("ventilation_kwh", env.ventilation_kwh),
            ("hvac_heating_kwh", env.hvac_heating_kwh),
            ("hvac_cooling_kwh", -env.hvac_cooling_kwh),
            ("internal_gains_kwh", env.internal_gains_kwh),
            ("duct_loss_kwh", env.duct_loss_kwh),
            ("internal_mass_kwh", env.internal_mass_kwh),
        ];
        for (name, value) in components {
            assert!(value.is_finite(), "{name} is not finite: {value}");
        }
        let signed_sum: f64 = components.iter().map(|(_, v)| v).sum();
        let abs_sum: f64 = components.iter().map(|(_, v)| v.abs()).sum();
        eprintln!(
            "  envelope budget: signed sum {signed_sum:+.3} kWh, sum|component| {abs_sum:.3} kWh"
        );

        assert!(
            abs_sum > 1.0,
            "sum|component| = {abs_sum:.3} kWh over a full summer day is degenerate: \
             the envelope accumulators are not running in this build configuration"
        );
        assert!(
            signed_sum.abs() <= 0.5 * abs_sum,
            "envelope budget does not close: signed sum {signed_sum:+.3} kWh vs \
             sum|component| {abs_sum:.3} kWh (ratio {:.2}). A ratio near 1 means a \
             positive-dominant gross flux (e.g. exterior solar+LWR absorption) is \
             wired into a field documented as a net zone load — the components \
             cannot be read as a budget",
            signed_sum.abs() / abs_sum
        );
    }

    /// Tripwire: the budget-closure test above enumerates the summable
    /// fields by name, so a field added to `EnvelopeComponentLoadsKwh`
    /// would otherwise escape the balance assertion silently — which is how
    /// a gross-vs-net mislabel enters this struct. The struct has no field
    /// reflection, so count the `_kwh` members in its `Debug` form and pin
    /// the count: adding a field fails here with instructions, forcing the
    /// author to either wire the new field into
    /// `envelope_load_budget_is_signed_summable_and_closes` (and
    /// `envelope_load_fields_equal_their_csv_column_integrals`) or document
    /// it inline as a gross, non-summable exception the way
    /// `interior_lwr_kwh` is.
    #[test]
    fn envelope_loads_field_count_pins_budget_membership() {
        let debug = format!("{:?}", EnvelopeComponentLoadsKwh::default());
        let field_count = debug.matches("_kwh:").count();
        assert_eq!(
            field_count, 11,
            "EnvelopeComponentLoadsKwh gained or lost a field (now {field_count}, \
             pinned at 11). A new summable net zone load must be added to the \
             components array in `envelope_load_budget_is_signed_summable_and_closes` \
             and to `envelope_load_fields_equal_their_csv_column_integrals`; a gross \
             or non-summable metric must carry an explicit \"not summable\" caveat in \
             its doc comment like `interior_lwr_kwh`. Then update this count."
        );
    }

    /// Per-field end-to-end wiring: every `EnvelopeComponentLoadsKwh` field
    /// must equal the time-integral of its own per-interval CSV output
    /// column from the same run. The struct is documented as the
    /// accumulation of the "... Heat Gain ... (W)" columns, so a field that
    /// disagrees with its column integral (wrong column bound, scale/unit
    /// error, accumulator drift) is a mis-wired field — whatever the signed
    /// budget bound says. The budget test only constrains the SUM: a single
    /// field sourced at 2–3x its true magnitude still closes the 0.5·Σ|c|
    /// bound, and the synthetic unit tests pin the name→field binding only
    /// on hand-built columns, never on real solver output end-to-end.
    #[test]
    fn envelope_load_fields_equal_their_csv_column_integrals() {
        let data = summer_24h_columns();
        let env = &summer_24h_run().1;

        let dt_h = 1.0 / 60.0; // 1-minute fixture timestep
        let integral_kwh =
            |name: &str| -> f64 { column_values(data, name).iter().sum::<f64>() * dt_h / 1000.0 };
        // ventilation_kwh accumulates BOTH ventilation columns.
        let ventilation_integral = integral_kwh("Forced Ventilation Heat Gain - Indoor (W)")
            + integral_kwh("Natural Ventilation Heat Gain - Indoor (W)");

        let cases: [(&str, f64, f64); 11] = [
            (
                "window_solar_kwh",
                env.window_solar_kwh,
                integral_kwh("Window Transmitted Solar Gain (W)"),
            ),
            (
                "window_conduction_kwh",
                env.window_conduction_kwh,
                integral_kwh("Window Heat Gain - Indoor (W)"),
            ),
            (
                "opaque_conduction_kwh",
                env.opaque_conduction_kwh,
                integral_kwh("Opaque Surface Heat Gain - Indoor (W)"),
            ),
            (
                "internal_mass_kwh",
                env.internal_mass_kwh,
                integral_kwh("Internal Mass Heat Gain - Indoor (W)"),
            ),
            (
                "interior_lwr_kwh",
                env.interior_lwr_kwh,
                integral_kwh("Interior LWR Exchange - Indoor (W)"),
            ),
            (
                "infiltration_kwh",
                env.infiltration_kwh,
                integral_kwh("Infiltration Heat Gain - Indoor (W)"),
            ),
            ("ventilation_kwh", env.ventilation_kwh, ventilation_integral),
            (
                "hvac_heating_kwh",
                env.hvac_heating_kwh,
                integral_kwh("HVAC Heating Delivered (W)"),
            ),
            (
                "hvac_cooling_kwh",
                env.hvac_cooling_kwh,
                integral_kwh("HVAC Cooling Delivered (W)"),
            ),
            (
                "internal_gains_kwh",
                env.internal_gains_kwh,
                integral_kwh("Internal Heat Gain - Indoor (W)"),
            ),
            (
                "duct_loss_kwh",
                env.duct_loss_kwh,
                integral_kwh("Duct Loss Heat Gain - Indoor (W)"),
            ),
        ];

        for (field, reported_kwh, expected_kwh) in cases {
            // The streaming calculator accumulates Σ(v·dt) row-by-row while
            // this check computes (Σv)·dt; both are round-trip-exact per
            // value (arrow-csv writes shortest round-trip floats), so the
            // only difference is summation order — ~1e-12 relative at this
            // row count. 1e-9 relative leaves six orders of margin while
            // still catching any real mis-wiring (≥ tens of percent).
            let tol = 1e-9 * expected_kwh.abs().max(1e-9);
            assert!(
                (reported_kwh - expected_kwh).abs() <= tol,
                "{field} = {reported_kwh:.9} kWh disagrees with the time-integral of \
                 its CSV column ({expected_kwh:.9} kWh) by {:.3e} kWh: the field is \
                 not accumulating the per-interval column it is documented to \
                 accumulate (wrong column binding, scale error, or accumulator drift)",
                (reported_kwh - expected_kwh).abs()
            );
        }
    }
}
