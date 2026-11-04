#![cfg(debug_assertions)]
//! The meter-versus-network reconciliation on the engine's own runs.
//!
//! The instrument's fields and accessors are `cfg(any(test, debug_assertions))`
//! on the library: a release build carries no receipt. This integration test
//! therefore compiles only when `debug_assertions` is on; a release clippy
//! sees an empty target instead of a reference to an absent accessor.
//!
//! The fan-out's end-use reconciliation closed the OCHRE fixture corpus's
//! meter columns three ways (the end-use aggregates against their
//! instances, the total against both). This row extends the same
//! reconciliation to runs the engine assembles itself, and closes the face
//! that corpus could not see: the delivered end-uses against the network
//! solve. Per step, three accounts must agree:
//!
//! 1. the meters: the per-instance electric (and gas) power columns, their
//!    end-use aggregates, and the totals;
//! 2. the end-uses' assignment: the run's energy-closure report's
//!    delivered column, on the system row and the per-zone rows;
//! 3. the network solve's receipt: the port-only input vector's
//!    capacitance-weighted account through the same operator the balance
//!    block applies to the full input vector (the discrete `B_d` when the
//!    step is uncoupled, its semi-implicit `(I + D)⁻¹·B_d` form when the
//!    step carries coupling entries), `ThermalSolver::port_receipt_w`.
//!
//! The meters-minus-receipt gap is exact, explained, and bounded. The
//! explanation is the receipt's own account: a port joule's response is
//! its stored share plus the conduction it drives to the boundary within
//! the step, because the outdoor and ground temperatures ride their own
//! input columns and the state matrix's columns therefore do not sum to
//! zero through the capacitances. Each port column's storage share takes
//! the `(1 − e^{−x})/x` shape below unity (`x` the column's
//! boundary-conduction time-constant ratio), and the solver publishes the
//! explained gap as the port input total minus the receipt, beside the
//! receipt. The reconciliation pins the exact residual (meters minus
//! receipt minus the gap) to zero, and bounds the gap itself at 5 percent
//! of the metered delivery. A dropped or duplicated port entry in the
//! input mapping moves the exact residual by its whole share and fails at
//! once; a dropped or duplicated input-matrix column moves the gap by that
//! column's whole share and fails the scale pin when the share exceeds the
//! bound. A change smaller than the bound is indistinguishable from the
//! physical within-step conduction the scale pin admits, by construction.
//!
//! The short-wave pass's glazing back-out is the second explained part.
//! The pass pools the port's visible gain with the transmitted diffuse
//! solar and the windows' transmittance share leaves back out through the
//! glazing (`HeatBalanceSurfaceManager.cc:3693`, `:3749-3753`), so the
//! meters carry joules the network legitimately never receives. The pass
//! returns the injected total and the back-out separately; a joule that
//! failed to inject is neither, so it stays in the exact residual instead
//! of being explained away as a glazing loss.
//!
//! The faces that legitimately do not close are published, not asserted
//! away. A storage buffer's state of charge absorbs the difference between
//! its metered flow and everything else its port account shows: the
//! reconciliation prints the flow integral beside the state-of-charge
//! trajectory and pins only what the buffer's own contract gives (the
//! channel is present, finite, in bounds, and moves in the flow's
//! direction on at least one material step). The deferred and unserved
//! class (the unmet HVAC load column, the ideal-capacity and rolled-back
//! counters, the island imbalance) is read and printed beside the delivered
//! account on every run, and its channels must be finite: an unexplained
//! gap is a reported column, never a suppressed one. The fleet's gas
//! fixture must burn gas, so a run that silently stopped writing the gas
//! columns cannot pass as "no gas here".
//!
//! No OCHRE corpus fixture is mounted here: every run is the engine's own
//! (two synthetic RC dwellings and four HPXML assemblies on the example
//! schedule), and no probe holds more than a handful of dwellings.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Array, Float64Array};
use arrow::record_batch::RecordBatch;
use hares_core::{Dwelling, DwellingConfig};
use hares_input::{DefaultsSource, InputSource, SimulationConfig, WeatherInput};

/// The hybrid tolerance: an absolute floor plus a share of the magnitude
/// scale. Adding n floating-point terms carries at most n ulps of the
/// largest partial sum (addition is commutative, not associative, and the
/// accounts' summation orders differ), so the bound is
/// `1e-9 + 1e-9 · Σ|terms|`: at n = 30 the worst-case n-ulp residual is
/// about 6.7e-15 of the scale (30 · 2^-52), which the relative floor
/// exceeds by about 1.5e5, and the absolute floor covers all-zero sums
/// (cancellation makes a relative form meaningless against a near-zero
/// total).
fn close_sum(sum: f64, terms: &[f64], what: &str, failures: &mut Vec<String>) {
    let scale: f64 = terms.iter().map(|t| t.abs()).sum();
    let residual = (sum - terms.iter().sum::<f64>()).abs();
    if residual > 1e-9 + 1e-9 * scale {
        failures.push(format!(
            "{what}: sum {sum} differs from [{}] by {residual} (scale {scale})",
            terms
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

/// The two-account form of the same tolerance: the residual between two
/// independently accumulated totals against the pair's own scale.
fn close_pair(a: f64, b: f64, what: &str, failures: &mut Vec<String>) {
    let residual = (a - b).abs();
    if residual > 1e-9 + 1e-9 * (a.abs() + b.abs()) {
        failures.push(format!("{what}: {a} differs from {b} by {residual}"));
    }
}

/// The run frame's per-step columns: every non-Time column a
/// `Vec<Option<f64>>` over the rows, so per-step identities line up even
/// where a nullable column carries its documented pre-reconcile nulls.
fn read_batch_columns(batches: &[RecordBatch]) -> BTreeMap<String, Vec<Option<f64>>> {
    let mut columns: BTreeMap<String, Vec<Option<f64>>> = BTreeMap::new();
    for batch in batches {
        let schema = batch.schema();
        for (idx, field) in schema.fields().iter().enumerate() {
            if field.name() == "Time" {
                continue;
            }
            let Some(float_array) = batch.column(idx).as_any().downcast_ref::<Float64Array>()
            else {
                continue;
            };
            let values = columns.entry(field.name().to_string()).or_default();
            for row in 0..float_array.len() {
                values.push(float_array.is_valid(row).then(|| float_array.value(row)));
            }
        }
    }
    columns
}

/// One equipment instance's side of the reconciliation: the column prefix
/// (the instance name the columns are written under) and the end use the
/// rollup groups it by.
struct Instance {
    name: String,
    end_use: String,
    end_use_is_battery: bool,
}

fn roster(dwelling: &Dwelling) -> Vec<Instance> {
    dwelling
        .equipment()
        .iter()
        .map(|eq| {
            let d = eq.descriptor();
            Instance {
                name: d.name.clone(),
                end_use: d.end_use.display_name().to_string(),
                end_use_is_battery: d.end_use == hares_types::EndUse::BATTERY,
            }
        })
        .collect()
}

/// The per-step reconciliation state: the previous cumulative delivered
/// totals, so each step's own delivery reads as a delta.
struct DeliveredBaseline {
    system: f64,
    zones: BTreeMap<hares_types::ZoneId, f64>,
}

impl DeliveredBaseline {
    fn new() -> Self {
        Self {
            system: 0.0,
            zones: BTreeMap::new(),
        }
    }
}

/// Steps one production step and reconciles the per-step faces every
/// engine run owes:
///
/// - the network's receipt against the meters: the closure report's
///   delivered delta (the ports' metered delivery, integrated) against the
///   solver's `port_receipt_w` (the network solve's own account of the
///   same delivery through the operator the step applies). The account is
///   pinned twice: the exact residual (meters minus receipt minus the
///   explained gap, the port input total minus the receipt plus the
///   short-wave glazing back-out) must be zero to the products' float
///   rounding, and the explained gap itself must stay within the physical
///   scale the column shape admits (the `(1 − e^{−x})/x` storage share
///   bounds each column's within-step loss at `x/2`, plus the coupling
///   damping on a coupled step; the bound here is 5 percent of the
///   metered delivery, above the measured 2.1 percent). A dropped or
///   duplicated port entry in the input mapping moves the exact residual
///   by its whole share; a dropped or duplicated input-matrix column
///   moves the gap by that column's whole share and fails the scale pin
///   when the share exceeds the bound.
/// - the end-uses' assignment: the per-zone rows' delivered deltas against
///   the system row's (the zones' own wiring closing on the system's).
///
/// Returns the step's (metered Wh, receipt Wh) pair for the caller's
/// account, and pushes any violation onto `failures`.
fn step_and_reconcile(
    dwelling: &mut Dwelling,
    baseline: &mut DeliveredBaseline,
    dt_s: f64,
    step: usize,
    failures: &mut Vec<String>,
) -> (f64, f64) {
    dwelling
        .step()
        .unwrap_or_else(|err| panic!("step {step} must succeed: {err}"));

    let closure = &dwelling.health().energy_closure;
    let metered_wh = closure.system.delivered_end_use_wh - baseline.system;
    let receipt_wh = dwelling.thermal_solver().port_receipt_w() * dt_s / 3600.0;
    let gap_wh = dwelling.thermal_solver().port_receipt_gap_w() * dt_s / 3600.0;
    close_pair(
        metered_wh - receipt_wh,
        gap_wh,
        &format!("step {step}: the meters-minus-receipt gap against its explained gap"),
        failures,
    );
    if (metered_wh - receipt_wh).abs() > 1e-9 + 0.05 * metered_wh.abs() {
        failures.push(format!(
            "step {step}: the meters-minus-receipt gap {} Wh exceeds the physical \
             within-step conduction scale (5% of the metered {metered_wh} Wh)",
            metered_wh - receipt_wh
        ));
    }

    let mut zone_deltas: Vec<(hares_types::ZoneId, f64)> = Vec::new();
    for row in &closure.zones {
        let previous = baseline.zones.insert(row.zone, row.delivered_end_use_wh);
        let delta = row.delivered_end_use_wh - previous.unwrap_or(0.0);
        zone_deltas.push((row.zone, delta));
    }
    let zone_ids: Vec<String> = zone_deltas.iter().map(|(z, _)| z.to_string()).collect();
    close_sum(
        metered_wh,
        &zone_deltas.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
        &format!(
            "step {step}: the per-zone delivered assignment against the system row (zones: {})",
            zone_ids.join(", ")
        ),
        failures,
    );

    baseline.system = closure.system.delivered_end_use_wh;
    (metered_wh, receipt_wh)
}

/// Prints the run's reconciliation account: the published column the
/// honesty contract owes for every face, closing or not.
fn print_account(fixture: &str, steps: usize, account: &str, value: String) {
    eprintln!("[reconciliation] fixture={fixture} steps={steps} {account} {value}");
}

/// The synthetic RC dwelling's TOML, on the minimal single-zone assembly
/// the energy-closure witnesses use: one wall and one floor boundary with
/// a material layer each (live node capacitances), constant 200 W internal
/// gains through the thermal ports, 60 steps of 60 s. `ach` selects the
/// infiltration rate, and with it whether the per-step semi-implicit
/// coupling carries entries.
fn synthetic_rc_dwelling_toml(ach: f64) -> String {
    format!(
        r#"building_id = 999

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res = 60
duration = 3600
master_seed = 0
[geometry]
number_of_bedrooms = 3.0
floor_area_m2 = 48.0
zone_volume_m3 = 129.6

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = 30.0
dew_point_c = 5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = {ach}

internal_gains_w = 200.0
internal_gains_constant = true
internal_gains_sensible_fraction = 1.0
internal_gains_radiant_fraction = 0.3

[[boundaries]]
id = "test-wall"
boundary_type = "Wall"
area_m2 = 48.0

[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.5
density_kg_m3 = 1000.0
specific_heat_j_kg_k = 1000.0

[[boundaries]]
id = "test-floor"
boundary_type = "Floor"
area_m2 = 48.0

[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.5
density_kg_m3 = 1000.0
specific_heat_j_kg_k = 1000.0

[schedule]
occupancy = 0.0
occupants_present = false
"#
    )
}

/// Builds the synthetic dwelling from the TOML text.
fn synthetic_dwelling(ach: f64) -> Dwelling {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("dwelling.toml");
    fs::write(&path, synthetic_rc_dwelling_toml(ach)).expect("write toml");
    Dwelling::from_toml_config(&path).expect("create dwelling")
}

const SYNTHETIC_STEPS: usize = 60;
const SYNTHETIC_DT_S: f64 = 60.0;

/// FACE: the network's receipt on the uncoupled assembly. Every step's
/// port-only `B_d` account must equal the ports' metered delivery minus
/// the columns' own explained within-step conduction, and the per-zone
/// assignment must close on the system row. The derivation in the module
/// comment is the proof the bounds ride; a dropped or duplicated port
/// entry in the discrete input mapping moves the exact residual by its
/// whole share, and a dropped or duplicated input-matrix column moves the
/// gap by that column's whole share.
#[test]
fn network_receipt_closes_against_the_port_meters_uncoupled() {
    let mut dwelling = synthetic_dwelling(0.0);
    let mut baseline = DeliveredBaseline::new();
    let mut failures: Vec<String> = Vec::new();
    let mut metered_total = 0.0;
    let mut receipt_total = 0.0;
    for step in 0..SYNTHETIC_STEPS {
        let (metered_wh, receipt_wh) = step_and_reconcile(
            &mut dwelling,
            &mut baseline,
            SYNTHETIC_DT_S,
            step,
            &mut failures,
        );
        metered_total += metered_wh;
        receipt_total += receipt_wh;
    }
    print_account(
        "synthetic-rc-ach0",
        SYNTHETIC_STEPS,
        "network-receipt:",
        format!(
            "meters {metered_total} Wh, receipt {receipt_total} Wh, gap {} Wh",
            metered_total - receipt_total
        ),
    );
    assert!(
        failures.is_empty(),
        "the uncoupled run's reconciliation violations:\n{}",
        failures.join("\n")
    );
}

/// FACE: the network's receipt on the coupled assembly. The semi-implicit
/// infiltration coupling changes the operator the step applies: the input
/// response is `(I + D)⁻¹·B_d`, so the receipt is computed through the
/// same damped operator the balance block uses and the damping joins the
/// explained gap. A receipt that ignored the coupling would report the
/// undamped `B_d` account and hide the damping; the differential witness
/// below pins that distinction.
#[test]
fn network_receipt_closes_against_the_port_meters_on_the_coupled_assembly() {
    let mut dwelling = synthetic_dwelling(0.5);
    let mut baseline = DeliveredBaseline::new();
    let mut failures: Vec<String> = Vec::new();
    let mut metered_total = 0.0;
    let mut receipt_total = 0.0;
    for step in 0..SYNTHETIC_STEPS {
        let (metered_wh, receipt_wh) = step_and_reconcile(
            &mut dwelling,
            &mut baseline,
            SYNTHETIC_DT_S,
            step,
            &mut failures,
        );
        metered_total += metered_wh;
        receipt_total += receipt_wh;
    }
    print_account(
        "synthetic-rc-ach0.5",
        SYNTHETIC_STEPS,
        "network-receipt:",
        format!(
            "meters {metered_total} Wh, receipt {receipt_total} Wh, gap {} Wh",
            metered_total - receipt_total
        ),
    );
    assert!(
        failures.is_empty(),
        "the coupled run's reconciliation violations:\n{}",
        failures.join("\n")
    );
}

/// FACE: the receipt is damped on the coupled assembly. The two synthetic
/// dwellings carry the same 200 W port input; the only difference is the
/// 0.5 ACH infiltration, whose semi-implicit coupling divides the air
/// node's input response by `1 + d` with `d = h_inf·dt/C_air` (about
/// 8e-3 here: 0.5 ACH over 129.6 m3 against the 60 s step). The coupled
/// receipt must therefore be strictly below the uncoupled `B_d` account.
/// This is the witness the coupled face alone cannot give: without it a
/// receipt that silently ran the uncoupled operator would pass every
/// identity pin (the gap is built from the same operator either way).
#[test]
fn the_coupled_receipt_is_damped_below_the_uncoupled_account() {
    let mut totals = Vec::new();
    for ach in [0.0, 0.5] {
        let mut dwelling = synthetic_dwelling(ach);
        let mut baseline = DeliveredBaseline::new();
        let mut failures: Vec<String> = Vec::new();
        let mut metered_total = 0.0;
        let mut receipt_total = 0.0;
        for step in 0..SYNTHETIC_STEPS {
            let (metered_wh, receipt_wh) = step_and_reconcile(
                &mut dwelling,
                &mut baseline,
                SYNTHETIC_DT_S,
                step,
                &mut failures,
            );
            metered_total += metered_wh;
            receipt_total += receipt_wh;
        }
        assert!(
            failures.is_empty(),
            "the ach {ach} run's reconciliation violations:\n{}",
            failures.join("\n")
        );
        totals.push((metered_total, receipt_total));
    }
    let (uncoupled_metered, uncoupled_receipt) = totals[0];
    let (coupled_metered, coupled_receipt) = totals[1];
    assert!(
        (uncoupled_metered - coupled_metered).abs() < 1e-9,
        "the same 200 W input must meter the same on both assemblies: {uncoupled_metered} against {coupled_metered}"
    );
    let damping_share = (uncoupled_receipt - coupled_receipt) / uncoupled_receipt;
    assert!(
        damping_share > 1e-4,
        "the coupled receipt must carry the coupling's damping: {coupled_receipt} against {uncoupled_receipt} \
         (share {damping_share}, the derived d is about 8e-3, so a damped receipt sits about 1e-3 below)"
    );
}

/// The HPXML assemblies the fleet face runs: the engine's own end-to-end
/// runs on the example schedule, two days at 3600 s, warm-up disabled, the
/// verbosity-8 frame in memory. Between them they carry every metered
/// face: all-electric (`base`), gas (`base-dhw-multiple`), the storage
/// buffer under a self-consumption BMS (`base-pv-battery`, whose actor
/// charges from the PV surplus and discharges on the evening load), and
/// the multi-zone assignment (`base-foundation-basement-garage`, whose
/// water heater's jacket losses land in the non-indoor zones' rows). The
/// override text is the fixture's own equipment override (empty for the
/// fixtures that need none).
const FLEET: [(&str, &str, &str); 4] = [
    ("base.xml", "all-electric", ""),
    ("base-dhw-multiple.xml", "gas", ""),
    (
        "base-pv-battery.xml",
        "storage",
        r#"{"Battery": {"bms_mode": "{\"SelfConsumption\":{\"min_soc\":0.1,\"max_soc\":1.0,\"solar_only_charging\":true}}"}}"#,
    ),
    ("base-foundation-basement-garage.xml", "multi-zone", ""),
];

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds and runs one fleet dwelling with the per-step reconciliation
/// riding the step loop. The run's shape is the end-use rollup's: two
/// days hourly, warm-up disabled, the verbosity-8 memory frame.
struct FleetRun {
    dwelling: Dwelling,
    result: Arc<hares_core::RunResult>,
    /// The per-step (metered Wh, receipt Wh) pairs the loop recorded.
    step_accounts: Vec<(f64, f64)>,
    /// The step's length in hours.
    dt_h: f64,
}

fn run_fleet_member(hpxml_name: &str, override_text: &str) -> FleetRun {
    let overrides = if override_text.is_empty() {
        None
    } else {
        Some(
            serde_json::from_str::<serde_json::Value>(override_text)
                .unwrap_or_else(|err| panic!("{hpxml_name}: the fixture's override parses: {err}")),
        )
    };
    let config = DwellingConfig {
        hpxml: InputSource::Path(
            project_root()
                .join("tests/fixtures/hpxml/ochre_samples")
                .join(hpxml_name),
        ),
        schedule: Some(InputSource::Path(
            project_root().join("data/examples/BEopt_example_schedule.csv"),
        )),
        weather: WeatherInput::Path(
            project_root().join("tests/fixtures/weather/test_location.epw"),
        ),
        defaults: DefaultsSource::directory(project_root().join("python/hares/defaults")),
        settings: hares_core::DwellingSettings::new(
            SimulationConfig::from_toml(
                "start_time = \"2023-01-01T00:00:00-07:00\"\n\
                 duration = 172800\n\
                 time_res = 3600\n\
                 warmup = \"disabled\"\n\
                 output_chunk_size = 64\n\
                 \n\
                 [[reports]]\n\
                 name = \"frame\"\n\
                 selection = { verbosity = 8 }\n\
                 destination = \"memory\"\n",
            )
            .expect("the reconciliation run's config parses"),
            overrides,
            100,
            None,
        ),
    };
    let mut dwelling =
        Dwelling::from_config(config).unwrap_or_else(|err| panic!("assemble {hpxml_name}: {err}"));

    let dt_s = dwelling.sim_config().time_res.num_seconds() as f64;
    let total_steps = {
        let sim = dwelling.sim_config();
        (sim.duration.num_seconds() / sim.time_res.num_seconds()) as usize
    };

    let mut baseline = DeliveredBaseline::new();
    let mut failures: Vec<String> = Vec::new();
    let mut step_accounts: Vec<(f64, f64)> = Vec::with_capacity(total_steps);
    for step in 0..total_steps {
        let (metered_wh, receipt_wh) =
            step_and_reconcile(&mut dwelling, &mut baseline, dt_s, step, &mut failures);
        step_accounts.push((metered_wh, receipt_wh));
    }
    assert!(
        failures.is_empty(),
        "{hpxml_name}: the per-step reconciliation violations:\n{}",
        failures.join("\n")
    );

    let result = dwelling
        .finish()
        .unwrap_or_else(|err| panic!("finish {hpxml_name}: {err}"));
    FleetRun {
        dwelling,
        result,
        step_accounts,
        dt_h: dt_s / 3600.0,
    }
}

/// FACES: the meters, the storage buffers and the unserved class on the
/// engine's own end-to-end runs, beside the per-step faces the loop above
/// already asserted.
///
/// - the electric meters close three ways per step (the end-use aggregates
///   against their instances, the total against both);
/// - the gas meters close the same way where the run burns gas;
/// - the storage buffer's state of charge is published beside its metered
///   flow integral (the buffer's own conversion account, never asserted
///   to zero), and the channel is present, finite, in bounds, and moves in
///   the flow's direction on at least one material step (a frozen state of
///   charge under a live meter fails);
/// - the unserved and deferred class is read and printed on every run:
///   the unmet HVAC load column, the ideal-capacity and rolled-back-step
///   counters, the island imbalance.
#[test]
fn engine_runs_reconcile_meters_end_uses_and_the_network_solve() {
    let mut all_failures: Vec<String> = Vec::new();
    let mut gas_face_seen = false;
    for (hpxml_name, shape, override_text) in FLEET {
        let FleetRun {
            dwelling,
            result,
            step_accounts,
            dt_h,
        } = run_fleet_member(hpxml_name, override_text);
        print_account(hpxml_name, step_accounts.len(), "shape:", shape.to_string());
        let mut failures: Vec<String> = Vec::new();

        let batches = result.reports[0].batches();
        let columns = read_batch_columns(batches);
        let row_count = step_accounts.len();
        let frame_rows = columns.values().next().map(Vec::len).unwrap_or(0);
        if frame_rows != row_count {
            failures.push(format!(
                "the frame's row count {frame_rows} must equal the stepped count {row_count}"
            ));
        }

        // The meter side's roster, from the dwelling's own equipment.
        let instances = roster(&dwelling);
        let mut end_use_groups: Vec<(String, Vec<&Instance>)> = Vec::new();
        for instance in &instances {
            match end_use_groups
                .iter_mut()
                .find(|(name, _)| *name == instance.end_use)
            {
                Some((_, members)) => members.push(instance),
                None => end_use_groups.push((instance.end_use.clone(), vec![instance])),
            }
        }

        let cell = |columns: &BTreeMap<String, Vec<Option<f64>>>, name: &str, row: usize| {
            columns
                .get(name)
                .and_then(|c| c.get(row))
                .copied()
                .flatten()
        };

        // FACE: the electric meters close three ways, per step.
        let aggregate_names: Vec<String> = end_use_groups
            .iter()
            .map(|(end_use, _)| format!("{end_use} End Use Electric Power (kW)"))
            .collect();
        for aggregate_name in &aggregate_names {
            if !columns.contains_key(aggregate_name) {
                failures.push(format!("no aggregate column '{aggregate_name}'"));
            }
        }
        for row in 0..row_count {
            let total = cell(&columns, "Total Electric Power (kW)", row).unwrap_or_else(|| {
                failures.push(format!("row {row}: null total electric cell"));
                0.0
            });
            let aggregate_terms: Option<Vec<f64>> = aggregate_names
                .iter()
                .map(|name| cell(&columns, name, row))
                .collect();
            match aggregate_terms {
                Some(terms) => close_sum(
                    total,
                    &terms,
                    &format!("{hpxml_name}: row {row}: total vs end-use aggregates"),
                    &mut failures,
                ),
                None => failures.push(format!("{hpxml_name}: row {row}: null aggregate cell")),
            }
            let instance_terms: Option<Vec<f64>> = instances
                .iter()
                .map(|instance| {
                    cell(
                        &columns,
                        &format!("{} Electric Power (kW)", instance.name),
                        row,
                    )
                })
                .collect();
            match instance_terms {
                Some(terms) => close_sum(
                    total,
                    &terms,
                    &format!("{hpxml_name}: row {row}: total vs instance meters"),
                    &mut failures,
                ),
                None => failures.push(format!("{hpxml_name}: row {row}: null instance meter")),
            }
        }
        let metered_total_kwh: f64 = step_accounts.iter().map(|(m, _)| m).sum::<f64>() / 1000.0;
        print_account(
            hpxml_name,
            row_count,
            "electric-meters:",
            format!(
                "delivered-end-uses {metered_total_kwh:.6} kWh across {} instance(s), {} end use(s)",
                instances.len(),
                end_use_groups.len()
            ),
        );

        // FACE: the gas meters close the same way where the run burns gas.
        // The fleet carries a gas fixture, so the face must be exercised by
        // at least one member: a run that silently stopped writing the gas
        // columns would otherwise pass every check as "no gas here".
        let gas_instances: Vec<&Instance> = instances
            .iter()
            .filter(|instance| {
                columns.contains_key(&format!("{} Gas Power (therms/hour)", instance.name))
            })
            .collect();
        if !gas_instances.is_empty() {
            gas_face_seen = true;
            match columns.get("Total Gas Power (therms/hour)") {
                Some(gas_total_column) => {
                    for row in 0..row_count {
                        let total = gas_total_column
                            .get(row)
                            .copied()
                            .flatten()
                            .unwrap_or_else(|| {
                                failures.push(format!("row {row}: null total gas cell"));
                                0.0
                            });
                        let terms: Option<Vec<f64>> = gas_instances
                            .iter()
                            .map(|instance| {
                                cell(
                                    &columns,
                                    &format!("{} Gas Power (therms/hour)", instance.name),
                                    row,
                                )
                            })
                            .collect();
                        match terms {
                            Some(terms) => close_sum(
                                total,
                                &terms,
                                &format!("{hpxml_name}: row {row}: total gas"),
                                &mut failures,
                            ),
                            None => {
                                failures.push(format!("{hpxml_name}: row {row}: null gas meter"))
                            }
                        }
                    }
                }
                None => failures.push(format!(
                    "{hpxml_name}: gas-burning instances are present but the total gas column is missing"
                )),
            }
        }
        print_account(
            hpxml_name,
            row_count,
            "gas-meters:",
            if gas_instances.is_empty() {
                "no gas-burning instance on this run".to_string()
            } else {
                format!(
                    "{} gas-burning instance(s) close against the total",
                    gas_instances.len()
                )
            },
        );

        // FACE: the storage buffers publish their state-of-charge account
        // beside the metered flow. The buffer's conversion account (the
        // round-trip difference) is the buffer's own; the reconciliation
        // publishes both sides and pins only the channel's contract and
        // the flow's sign on material steps.
        let battery_instances: Vec<&Instance> = instances
            .iter()
            .filter(|instance| instance.end_use_is_battery)
            .collect();
        for instance in battery_instances {
            let soc_name = format!("{} SOC (-)", instance.name);
            let Some(soc_column) = columns.get(&soc_name) else {
                failures.push(format!("no state-of-charge column '{soc_name}'"));
                continue;
            };
            let power_name = format!("{} Electric Power (kW)", instance.name);
            let mut flow_kwh = 0.0;
            let mut soc_first: Option<f64> = None;
            let mut soc_last: Option<f64> = None;
            let mut previous_soc: Option<f64> = None;
            let mut sign_violations = 0;
            let mut material_steps = 0;
            let mut sign_matches = 0;
            for row in 0..row_count {
                let soc = soc_column.get(row).copied().flatten();
                match soc {
                    Some(value) => {
                        if !(0.0..=1.0).contains(&value) {
                            failures
                                .push(format!("row {row}: '{soc_name}' outside [0, 1]: {value}"));
                        }
                        if soc_first.is_none() {
                            soc_first = Some(value);
                        }
                        soc_last = Some(value);
                        let power = cell(&columns, &power_name, row).unwrap_or(0.0);
                        if power.abs() > 1e-3
                            && let Some(previous) = previous_soc
                        {
                            material_steps += 1;
                            let stored_delta = value - previous;
                            if stored_delta * power < 0.0 {
                                sign_violations += 1;
                            } else if stored_delta * power > 0.0 {
                                sign_matches += 1;
                            }
                        }
                        previous_soc = Some(value);
                    }
                    None => failures.push(format!("row {row}: null cell in '{soc_name}'")),
                }
                flow_kwh += cell(&columns, &power_name, row).unwrap_or(0.0) * dt_h;
            }
            print_account(
                hpxml_name,
                row_count,
                "storage:",
                format!(
                    "'{}' metered flow {flow_kwh:.6} kWh, soc {:?} -> {:?}, \
                     sign violations {sign_violations} on {material_steps} material step(s), \
                     sign matches {sign_matches} (the state of charge is the \
                     buffer's own account of the flow's round trip)",
                    instance.name, soc_first, soc_last
                ),
            );
            if sign_violations > 0 {
                failures.push(format!(
                    "'{}': the state of charge moved against the flow's sign on \
                     {sign_violations} material step(s)",
                    instance.name
                ));
            }
            if material_steps > 0 && sign_matches == 0 {
                failures.push(format!(
                    "'{}': the state of charge never moved with the flow on \
                     {material_steps} material step(s)",
                    instance.name
                ));
            }
        }

        // FACE: the unserved and deferred class, published on every run.
        // The column need not move (a served run legitimately reads zero),
        // but it must be present and finite: a non-finite cell is a
        // reported violation, never folded into a maximum.
        let mut unmet_max = 0.0_f64;
        match columns.get("Unmet HVAC Load (C)") {
            Some(column) => {
                for (row, value) in column.iter().enumerate() {
                    if let Some(value) = value {
                        if value.is_finite() {
                            unmet_max = unmet_max.max(value.abs());
                        } else {
                            failures.push(format!(
                                "row {row}: 'Unmet HVAC Load (C)' carried a non-finite value: {value}"
                            ));
                        }
                    }
                }
            }
            None => {
                failures.push("no 'Unmet HVAC Load (C)' column in the frame".to_string());
            }
        }
        let health = dwelling.health();
        let island_unserved_w = dwelling.island_unserved_w();
        let island_excess_w = dwelling.island_excess_w();
        print_account(
            hpxml_name,
            row_count,
            "unserved:",
            format!(
                "unmet hvac load max {unmet_max:.6} C, ideal-capacity-unsolved steps {}, \
                 discarded-state-energy {} Wh, island-unserved {island_unserved_w} W, island-excess {island_excess_w} W",
                health.ideal_capacity_unsolved_steps, health.discarded_state_energy_wh,
            ),
        );
        if !health.discarded_state_energy_wh.is_finite() {
            failures.push(format!(
                "'discarded-state-energy' carried a non-finite value: {}",
                health.discarded_state_energy_wh
            ));
        }
        if !island_unserved_w.is_finite() {
            failures.push(format!(
                "the island-unserved channel carried a non-finite value: {island_unserved_w}"
            ));
        }
        if !island_excess_w.is_finite() {
            failures.push(format!(
                "the island-excess channel carried a non-finite value: {island_excess_w}"
            ));
        }

        let receipt_wh_total: f64 = step_accounts.iter().map(|(_, r)| r).sum();
        print_account(
            hpxml_name,
            row_count,
            "network-receipt:",
            format!(
                "meters {:.6} Wh, receipt {receipt_wh_total:.6} Wh, gap {} Wh",
                metered_total_kwh * 1000.0,
                metered_total_kwh * 1000.0 - receipt_wh_total
            ),
        );

        all_failures.extend(failures);
    }
    assert!(
        gas_face_seen,
        "the fleet's gas fixture never produced a gas-burning instance: the gas face was not exercised"
    );
    assert!(
        all_failures.is_empty(),
        "the fleet's reconciliation violations:\n{}",
        all_failures.join("\n")
    );
}
