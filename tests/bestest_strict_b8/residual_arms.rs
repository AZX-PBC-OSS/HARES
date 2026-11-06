//! The residual-attribution study's arms: one changed factor per run,
//! the annual delta recorded, no production number moved.
//!
//! Arm structure (the study's own contract):
//!
//! - **Control**: the production construction (StarMesh), rerun and
//!   pinned bitwise against the parent `5d5a73d0`'s P1 baseline. A moved
//!   control is a STOP: the study would not be measuring one factor.
//! - **Arm a - the LWR method**: the same case, the same fixture, the
//!   same inputs, built through the study's explicit
//!   `experimental-scriptf` construction with the ScriptF method named.
//!   The selector's fingerprint is asserted (the arm's interior-LWR
//!   injection activity is nonzero where the control's is identically
//!   zero, and the resolved read-back is unchanged), so the recorded
//!   annual delta is the method's response alone.
//! - **Arm b - the MRT linearization delta**: post-processing over the
//!   ScriptF arm's observed per-surface temperatures: the exact T⁴
//!   radiosity flux the arm's own diagnostics report, against the
//!   linearized star form the A-matrix bakes, evaluated at the same
//!   temperatures. No production MRT implementation; the post-processed
//!   delta decides whether an MRT variant is a follow-on row.
//! - **Arm c - the window's glass node**: the window interior conduction
//!   series per arm plus the ScriptF arm's window face state (the
//!   outdoor-anchored face temperature the LWR route drives). EnergyPlus
//!   solves per-glass-face temperatures
//!   (`WindowManager.cc:4731-4766`); the comparison's surface is the face
//!   state, never zone air alone.
//! - **Arm d - the ground**: the actual driven ground series' monthly
//!   means, recorded against the EPW's own monthly ground row and the
//!   BESTEST constant. Input identity only: a ground-policy replacement
//!   is not an arm unless the inputs first disagree.
//! - **Arm e - the TARP skip**: the withheld-correction count per case
//!   (the T-0034 limitation's measured reach) plus the film-regime
//!   identity.
//!
//! Every number the arms print is a measurement of the two runs named;
//! nothing here asserts a band, widens a band, or touches a drift lock.

use chrono::Datelike;
use hares_core::Dwelling;
use hares_core::ResolvedModel;
use hares_envelope::InteriorLwrMethod;
use hares_envelope::thermal_solver::FilmCoefficientModel;

use super::cases::{BestestCase, core_cases, in_depth_cases};

/// The study's case set: the conditioned and free-float mass pairs plus
/// the in-depth solid-conduction pair, exactly the rows the residual
/// study measures (600/900, 600FF/900FF, 195/395).
const ARM_CASE_IDS: [&str; 6] = ["600", "900", "600FF", "900FF", "195", "395"];

/// The reference temperature the linearized conductance bakes (the
/// star-mesh operating point, boundary_rc.rs's own constant).
const T_REF_K: f64 = 293.15;

/// The parent `5d5a73d0`'s P1 baseline at full precision: the controls'
/// bitwise pins, `(heating kWh, cooling kWh, peak C, min C)`. The
/// parent's own figures - captured by the strict selection's run at the
/// parent and the study's control capture, printed at round-trip
/// precision - are the values the drift-lock table's six-decimal record
/// rounds. The five strict-selection metrics' pins are the parent run's
/// own prints (the strict tests' RED/GREEN capture); the rest are the
/// control rerun's, identical bitwise because the knob's default path
/// changes no production byte.
fn pinned_control(case_id: &str) -> (f64, f64, f64, f64) {
    match case_id {
        "600" => (
            4033.9118828772553,
            5411.481455516629,
            35.59735759983536,
            20.079235047172308,
        ),
        "900" => (
            1342.6379642783231,
            2414.283990829509,
            31.085189651608207,
            19.996422928972333,
        ),
        "600FF" => (0.0, 0.0, 70.24815460460714, -12.002474434568812),
        "900FF" => (0.0, 0.0, 45.52845274667295, 2.3798816266459015),
        "195" => (
            3979.9672483804015,
            601.8190946633296,
            20.000000000000007,
            19.99999999999999,
        ),
        "395" => (
            4385.626147743442,
            11.988882528098072,
            28.28629643979463,
            19.99999999999999,
        ),
        _ => panic!("no control pin for case {case_id}"),
    }
}

/// One arm's annual record: the run's metrics and the per-step
/// diagnostics the study reads.
struct ArmRecord {
    case_id: &'static str,
    heating_kwh: f64,
    cooling_kwh: f64,
    peak_temp_c: f64,
    min_temp_c: f64,
    steps: u64,
    /// Interior-LWR injection activity integrated over the year
    /// (Σ|q|/2·dt, kWh): identically zero in StarMesh mode (the
    /// conductance is baked, nothing injects), the arm's fingerprint in
    /// ScriptF mode.
    interior_lwr_activity_kwh: f64,
    /// Window interior convection integrated signed and absolute
    /// (kWh), and the largest magnitude (W): the window's interior
    /// conduction series per arm (arm c).
    window_conduction_kwh_signed: f64,
    window_conduction_kwh_abs: f64,
    window_max_abs_w: f64,
    /// The per-boundary conduction series (kWh, signed): the walls',
    /// floor's and roof's interior-face convection the component gains
    /// report.
    wall_conduction_kwh: f64,
    floor_conduction_kwh: f64,
    roof_conduction_kwh: f64,
    /// Absorbed opaque exterior solar and the gross exterior LWR
    /// (kWh, signed), the exterior-skin drivers.
    opaque_solar_kwh: f64,
    exterior_lwr_kwh: f64,
    /// The driven ground series' monthly means (arm d) and its annual
    /// extremes.
    ground_monthly_mean_c: [f64; 12],
    ground_min_c: f64,
    ground_max_c: f64,
    /// The withheld-TARP-correction count summed over the year (arm e),
    /// and the film regime the run armed.
    tarp_negative_delta_skips: u64,
    film_model: FilmCoefficientModel,
    /// The zone-air balance residual's extremes (W): the run's own
    /// energy-closure account.
    closure_residual_max_abs_w: f64,
    closure_residual_sum_abs_w: f64,
    /// Arm b's post-processed linearization delta (ScriptF runs only):
    /// the exact T⁴ radiosity and the linearized star form at the same
    /// observed surface temperatures, as annual exchange-activity
    /// integrals (kWh), the largest per-surface-step delta (W) and the
    /// mean per-surface-step delta (W).
    lwr_exact_activity_kwh: f64,
    lwr_linear_activity_kwh: f64,
    lwr_delta_max_abs_w: f64,
    lwr_delta_mean_abs_w: f64,
    /// Arm c's ScriptF window face state (ScriptF runs with window
    /// surfaces only): the window rows' LWR flux activity (kWh) and the
    /// face temperatures' annual extremes.
    window_lwr_activity_kwh: Option<f64>,
    window_face_temp_min_c: Option<f64>,
    window_face_temp_max_c: Option<f64>,
}

/// Runs one arm: the case's full year at the named interior-LWR method,
/// the per-step diagnostics accumulated as the run steps. Returns the
/// record and the assembly's resolved read-back (the arm's selector
/// proof).
fn run_arm(case: &BestestCase, method: InteriorLwrMethod) -> (ArmRecord, ResolvedModel) {
    let mut dwelling = match method {
        InteriorLwrMethod::StarMesh => {
            Dwelling::from_toml_config_with_reports(&case.fixture_path(), Some(Vec::new()))
        }
        InteriorLwrMethod::ScriptF => Dwelling::from_toml_config_with_interior_lwr_method(
            &case.fixture_path(),
            Some(Vec::new()),
            InteriorLwrMethod::ScriptF,
        ),
    }
    .unwrap_or_else(|err| panic!("case {} ({method:?}) builds: {err}", case.id));

    // The selector's fingerprint precondition: the run's config carries
    // exactly the named method.
    assert_eq!(
        dwelling.thermal_solver().config().interior_lwr_method,
        method,
        "case {}: the assembled solver must carry the named interior-LWR method",
        case.id
    );

    // Arm b's per-surface exchange factors, read from the assembled
    // config: the interior-LWR zone surfaces' areas and emissivities,
    // flattened in the config's own (zone, surface) order - the order
    // the per-surface diagnostics report.
    let lwr_surface_factors: Vec<(f64, f64)> = dwelling
        .thermal_solver()
        .config()
        .interior_lwr_zones
        .iter()
        .flat_map(|zone| zone.surfaces.iter().map(|s| (s.area_m2, s.emissivity)))
        .collect();
    // The window rows of the same list: the surfaces whose face state is
    // environmentally driven (the windows), arm c's comparison surface.
    let lwr_surface_is_window: Vec<bool> = dwelling
        .thermal_solver()
        .config()
        .interior_lwr_zones
        .iter()
        .flat_map(|zone| zone.surfaces.iter().map(|s| s.driving_temp.is_some()))
        .collect();
    assert_eq!(
        lwr_surface_factors.len(),
        lwr_surface_is_window.len(),
        "case {}: the surface factor and window-flag lists are parallel",
        case.id
    );
    let resolved = dwelling.resolved_model().clone();

    let dt_hours = case.timestep_seconds as f64 / 3600.0;
    let mut record = ArmRecord {
        case_id: case.id,
        heating_kwh: 0.0,
        cooling_kwh: 0.0,
        peak_temp_c: f64::NEG_INFINITY,
        min_temp_c: f64::INFINITY,
        steps: 0,
        interior_lwr_activity_kwh: 0.0,
        window_conduction_kwh_signed: 0.0,
        window_conduction_kwh_abs: 0.0,
        window_max_abs_w: 0.0,
        wall_conduction_kwh: 0.0,
        floor_conduction_kwh: 0.0,
        roof_conduction_kwh: 0.0,
        opaque_solar_kwh: 0.0,
        exterior_lwr_kwh: 0.0,
        ground_monthly_mean_c: [0.0; 12],
        ground_min_c: f64::INFINITY,
        ground_max_c: f64::NEG_INFINITY,
        tarp_negative_delta_skips: 0,
        film_model: dwelling.thermal_solver().config().film_coefficient_model,
        closure_residual_max_abs_w: 0.0,
        closure_residual_sum_abs_w: 0.0,
        lwr_exact_activity_kwh: 0.0,
        lwr_linear_activity_kwh: 0.0,
        lwr_delta_max_abs_w: 0.0,
        lwr_delta_mean_abs_w: 0.0,
        window_lwr_activity_kwh: None,
        window_face_temp_min_c: None,
        window_face_temp_max_c: None,
    };

    // Arm d's monthly accumulator: the driven ground series' sums and
    // counts by calendar month.
    let mut ground_month_sums = [0.0_f64; 12];
    let mut ground_month_counts = [0_u64; 12];
    // Arm b's per-surface-step delta accumulator.
    let mut lwr_delta_count = 0_u64;
    let mut lwr_delta_sum_abs = 0.0_f64;

    let total_steps = dwelling.clock.total_steps();
    for _ in 0..total_steps {
        let step = dwelling
            .step()
            .unwrap_or_else(|err| panic!("case {} ({method:?}) steps: {err}", case.id));
        for (_, temp_c) in &step.zone_temperatures_c {
            record.peak_temp_c = record.peak_temp_c.max(*temp_c);
            record.min_temp_c = record.min_temp_c.min(*temp_c);
        }
        record.heating_kwh += step.hvac_heating_w / 1000.0 * dt_hours;
        record.cooling_kwh += step.hvac_cooling_w / 1000.0 * dt_hours;

        let gains = dwelling.thermal_solver().component_gains();
        record.interior_lwr_activity_kwh += gains.interior_lwr_w.abs() * dt_hours / 1000.0;
        record.window_conduction_kwh_signed += gains.window_heat_gain_w * dt_hours / 1000.0;
        record.window_conduction_kwh_abs += gains.window_heat_gain_w.abs() * dt_hours / 1000.0;
        record.window_max_abs_w = record.window_max_abs_w.max(gains.window_heat_gain_w.abs());
        record.wall_conduction_kwh += gains.wall_heat_gain_w * dt_hours / 1000.0;
        record.floor_conduction_kwh += gains.floor_heat_gain_w * dt_hours / 1000.0;
        record.roof_conduction_kwh += gains.roof_heat_gain_w * dt_hours / 1000.0;
        record.opaque_solar_kwh += gains.opaque_solar_w * dt_hours / 1000.0;
        record.exterior_lwr_kwh += gains.exterior_lwr_w * dt_hours / 1000.0;
        record.tarp_negative_delta_skips += gains.tarp_negative_delta_skips;
        record.closure_residual_max_abs_w = record
            .closure_residual_max_abs_w
            .max(gains.zone_air_balance_residual_w.abs());
        record.closure_residual_sum_abs_w += gains.zone_air_balance_residual_w.abs();

        let month = step.timestamp.month() as usize - 1;
        ground_month_sums[month] += gains.driving_ground_temp_c;
        ground_month_counts[month] += 1;
        record.ground_min_c = record.ground_min_c.min(gains.driving_ground_temp_c);
        record.ground_max_c = record.ground_max_c.max(gains.driving_ground_temp_c);

        // Arm b and arm c read the per-surface diagnostics: populated in
        // ScriptF mode only (the StarMesh mode injects nothing, so the
        // buffer stays empty - the control's absence is the baked form's
        // own fingerprint).
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        if method == InteriorLwrMethod::ScriptF {
            let diag = &gains.int_surface_diag;
            assert_eq!(
                diag.len(),
                lwr_surface_factors.len(),
                "case {}: the per-surface diagnostics run parallel to the \
                 interior-LWR surface list",
                case.id
            );
            // The linearized star form at the observed temperatures: the
            // conductance the A-matrix bakes (boundary_rc.rs's
            // G = linearised_h_rad(ε, T_ref)·A), the star node the Y-Δ
            // elimination preserves exactly (a linear network's exact
            // algebra), evaluated at the same temperatures the exact T⁴
            // radiosity's flux was evaluated at.
            let mut total_g = 0.0_f64;
            let mut total_gt = 0.0_f64;
            for (diag, &(area, emissivity)) in diag.iter().zip(lwr_surface_factors.iter()) {
                let g = hares_physics::constants::linearised_h_rad(emissivity, T_REF_K) * area;
                total_g += g;
                total_gt += g * diag.surface_temp_c;
            }
            for (i, diag) in diag.iter().enumerate() {
                let &(area, emissivity) = &lwr_surface_factors[i];
                let (t_surf, q_exact) = (diag.surface_temp_c, diag.lwr_flux_w);
                let g = hares_physics::constants::linearised_h_rad(emissivity, T_REF_K) * area;
                // The flux INTO the surface, the sign `lwr_flux_w` carries:
                // positive when the star node is warmer than the surface.
                // The repo's linearised form uses the same convention
                // (`interior_longwave_linearised_w`: T_mrt - T_surf).
                let q_linear = if total_g > 0.0 {
                    g * (total_gt / total_g - t_surf)
                } else {
                    0.0
                };
                let delta = (q_exact - q_linear).abs();
                record.lwr_exact_activity_kwh += q_exact.abs() * dt_hours / 1000.0 / 2.0;
                record.lwr_linear_activity_kwh += q_linear.abs() * dt_hours / 1000.0 / 2.0;
                record.lwr_delta_max_abs_w = record.lwr_delta_max_abs_w.max(delta);
                lwr_delta_sum_abs += delta;
                lwr_delta_count += 1;
                if lwr_surface_is_window[i] {
                    *record.window_lwr_activity_kwh.get_or_insert(0.0) +=
                        q_exact.abs() * dt_hours / 1000.0 / 2.0;
                    let min = record.window_face_temp_min_c.get_or_insert(t_surf);
                    *min = min.min(t_surf);
                    let max = record.window_face_temp_max_c.get_or_insert(t_surf);
                    *max = max.max(t_surf);
                }
            }
        }
        record.steps += 1;
    }

    for m in 0..12 {
        record.ground_monthly_mean_c[m] =
            ground_month_sums[m] / ground_month_counts[m].max(1) as f64;
    }
    record.lwr_delta_mean_abs_w = if lwr_delta_count > 0 {
        lwr_delta_sum_abs / lwr_delta_count as f64
    } else {
        0.0
    };
    (record, resolved)
}

/// The control's bitwise pin: the parent's P1 baseline reproduced to the
/// last bit on every metric the run records.
fn assert_control_bitwise(record: &ArmRecord) {
    let (heating, cooling, peak, min) = pinned_control(record.case_id);
    let checks: [(&str, f64, f64); 4] = [
        ("annual heating", record.heating_kwh, heating),
        ("annual cooling", record.cooling_kwh, cooling),
        ("peak zone temp", record.peak_temp_c, peak),
        ("min zone temp", record.min_temp_c, min),
    ];
    for (name, value, pinned) in checks {
        assert_eq!(
            value.to_bits(),
            pinned.to_bits(),
            "case {}: the control's {} {} moved off the parent's pinned \
             baseline {pinned} (bitwise): the study measures one factor and \
             a moved control is a STOP",
            record.case_id,
            name,
            value
        );
    }
}

/// The arm deltas' print: the causal table's row body, full precision.
fn print_arm_delta(control: &ArmRecord, arm: &ArmRecord) {
    eprintln!(
        "[arm-a] case={} control_heating={:?} arm_heating={:?} delta_heating={:+.6} kWh \
         control_cooling={:?} arm_cooling={:?} delta_cooling={:+.6} kWh \
         control_peak={:?} arm_peak={:?} delta_peak={:+.6} K \
         control_min={:?} arm_min={:?} delta_min={:+.6} K",
        control.case_id,
        control.heating_kwh,
        arm.heating_kwh,
        arm.heating_kwh - control.heating_kwh,
        control.cooling_kwh,
        arm.cooling_kwh,
        arm.cooling_kwh - control.cooling_kwh,
        control.peak_temp_c,
        arm.peak_temp_c,
        arm.peak_temp_c - control.peak_temp_c,
        control.min_temp_c,
        arm.min_temp_c,
        arm.min_temp_c - control.min_temp_c,
    );
    eprintln!(
        "[arm-a] case={} side effects: interior_lwr_activity control={:.6} arm={:.6} kWh; \
         closure residual max control={:.3} arm={:.3} W; steps control={} arm={}",
        control.case_id,
        control.interior_lwr_activity_kwh,
        arm.interior_lwr_activity_kwh,
        control.closure_residual_max_abs_w,
        arm.closure_residual_max_abs_w,
        control.steps,
        arm.steps,
    );
    eprintln!(
        "[arm-b] case={} linearization delta: exact activity={:.6} kWh, linearized \
         activity={:.6} kWh, max per-surface-step |dq|={:.3} W, mean={:.3} W",
        control.case_id,
        arm.lwr_exact_activity_kwh,
        arm.lwr_linear_activity_kwh,
        arm.lwr_delta_max_abs_w,
        arm.lwr_delta_mean_abs_w,
    );
    eprintln!(
        "[arm-c] case={} window: conduction signed control={:.6} arm={:.6} kWh, absolute \
         control={:.6} arm={:.6} kWh, max |q| control={:.3} arm={:.3} W; ScriptF window face \
         LWR activity={:?} kWh, face temp range {:?} C",
        control.case_id,
        control.window_conduction_kwh_signed,
        arm.window_conduction_kwh_signed,
        control.window_conduction_kwh_abs,
        arm.window_conduction_kwh_abs,
        control.window_max_abs_w,
        arm.window_max_abs_w,
        arm.window_lwr_activity_kwh,
        arm.window_face_temp_min_c.zip(arm.window_face_temp_max_c),
    );
    eprintln!(
        "[arm-d] case={} ground: monthly means {:?} C, annual range [{:.3}, {:.3}] C",
        control.case_id, control.ground_monthly_mean_c, control.ground_min_c, control.ground_max_c,
    );
    eprintln!(
        "[arm-e] case={} TARP: withheld-correction count control={} arm={} over {} steps; \
         film regime control={:?} arm={:?}; boundary conduction kWh \
         wall={:.3}/{:.3} floor={:.3}/{:.3} roof={:.3}/{:.3}; exterior drivers \
         solar={:.3}/{:.3} lwr={:.3}/{:.3} (control/arm)",
        control.case_id,
        control.tarp_negative_delta_skips,
        arm.tarp_negative_delta_skips,
        control.steps,
        control.film_model,
        arm.film_model,
        control.wall_conduction_kwh,
        arm.wall_conduction_kwh,
        control.floor_conduction_kwh,
        arm.floor_conduction_kwh,
        control.roof_conduction_kwh,
        arm.roof_conduction_kwh,
        control.opaque_solar_kwh,
        arm.opaque_solar_kwh,
        control.exterior_lwr_kwh,
        arm.exterior_lwr_kwh,
    );
}

/// The registry the arms read: the study's six cases, found.
fn arm_case(case_id: &str) -> BestestCase {
    core_cases()
        .into_iter()
        .chain(in_depth_cases())
        .find(|c| c.id == case_id)
        .unwrap_or_else(|| panic!("case {case_id} is in the study's case registry"))
}

/// The EPW's own monthly ground row (°C), parsed from the weather file's
/// `GROUND TEMPERATURES` header: the first depth set's twelve values.
/// Arm d's cited source quantity: the EPW parse prefers this row when the
/// file supplies it (`epw.rs`'s ground preference rule), and the BESTEST
/// cases share the one Denver file.
fn epw_monthly_ground_c() -> [f64; 12] {
    let fixture_root = super::cases::fixture_root();
    let epw_path = fixture_root
        .join("600.toml")
        .parent()
        .map(|dir| dir.join("../../../data/examples/USA_CO_Denver.epw"))
        .expect("the fixture root has a parent");
    let text = std::fs::read_to_string(&epw_path)
        .unwrap_or_else(|err| panic!("the Denver EPW reads: {err}"));
    let line = text
        .lines()
        .find(|l| l.starts_with("GROUND TEMPERATURES"))
        .expect("the Denver EPW carries a GROUND TEMPERATURES header row");
    let tokens: Vec<&str> = line.split(',').collect();
    // The row's shape: GROUND TEMPERATURES, <set count>, <depth>, ,,,,
    // <12 monthly values>, <depth 2>, ,,,, <12>, ... The first set's
    // twelve values start at token 6.
    const FIRST_SET_VALUES: usize = 6;
    let mut months = [0.0_f64; 12];
    for (m, month) in months.iter_mut().enumerate() {
        *month = tokens[FIRST_SET_VALUES + m]
            .trim()
            .parse::<f64>()
            .unwrap_or_else(|err| panic!("the EPW ground row's month {} parses: {err}", m + 1));
    }
    months
}

/// The BESTEST case definition's ground quantity (°C): the constant the
/// case spec declares, carried in-repo by the OCHRE runner's own
/// `Average Ground Temperature (C)` default (10.0,
/// scripts/ochre_bestest_600.py's location dict).
const BESTEST_CONSTANT_GROUND_C: f64 = 10.0;

/// One case's arm pair: the control rerun pinned bitwise, the ScriptF
/// arm's selector fingerprint proven (the resolved read-back unchanged,
/// the injection activity nonzero), the deltas printed.
fn run_case_arms(case_id: &str) {
    let case = arm_case(case_id);
    let (control, control_resolved) = run_arm(&case, InteriorLwrMethod::StarMesh);
    assert_control_bitwise(&control);

    // The selector's fingerprint, the control side: the production
    // construction injects nothing (the exchange is baked), so the
    // control's interior-LWR activity is identically zero. A nonzero
    // control activity would mean the production selection moved.
    assert_eq!(
        control.interior_lwr_activity_kwh, 0.0,
        "case {}: the control's interior-LWR injection activity must be \
         identically zero (StarMesh bakes the exchange)",
        case_id
    );
    assert_eq!(
        control.film_model,
        FilmCoefficientModel::AshraeSimple,
        "case {}: the production film regime is the frozen AshraeSimple \
         coefficient (the PerStepTarp correction is disabled by default)",
        case_id
    );

    let (arm, arm_resolved) = run_arm(&case, InteriorLwrMethod::ScriptF);
    assert_eq!(
        arm.film_model, control.film_model,
        "case {}: the arm changes the interior-LWR method and nothing else",
        case_id
    );
    // The resolved read-back's input identity is unchanged: the same
    // boundaries, windows, infiltration, site and clock the control
    // resolved. The method selection is not a resolved input; what the
    // read-back can legitimately carry differently is the autosized
    // equipment capacity, whose sizing solve runs through the assembled
    // envelope and so responds to the method - recorded below as a side
    // effect, never hidden behind an equality that could not hold.
    assert_eq!(
        arm_resolved.boundaries, control_resolved.boundaries,
        "case {}: the ScriptF arm's resolved boundaries must equal the \
         control's (the arm changes only the interior-LWR selector)",
        case_id
    );
    assert_eq!(
        arm_resolved.windows, control_resolved.windows,
        "case {}: the ScriptF arm's resolved windows must equal the control's",
        case_id
    );
    assert_eq!(
        arm_resolved.infiltration, control_resolved.infiltration,
        "case {}: the ScriptF arm's resolved infiltration must equal the \
         control's",
        case_id
    );
    assert_eq!(
        arm_resolved.site, control_resolved.site,
        "case {}: the ScriptF arm's resolved site must equal the control's",
        case_id
    );
    assert_eq!(
        arm_resolved.clock, control_resolved.clock,
        "case {}: the ScriptF arm's resolved clock must equal the control's",
        case_id
    );
    for (arm_eq, control_eq) in arm_resolved
        .equipment
        .iter()
        .zip(&control_resolved.equipment)
    {
        assert_eq!(
            (arm_eq.name.as_str(), arm_eq.id, &arm_eq.end_use),
            (control_eq.name.as_str(), control_eq.id, &control_eq.end_use),
            "case {}: the arm's equipment identity must equal the control's",
            case_id
        );
        for (arm_param, control_param) in arm_eq.parameters.iter().zip(&control_eq.parameters) {
            if arm_param != control_param {
                eprintln!(
                    "[arm-a] case={} side effect: autosized parameter {} moved \
                     {:?} to {:?} (the sizing solve runs through the assembled \
                     envelope, so the method responds in it)",
                    case_id, arm_param.name, control_param.value, arm_param.value
                );
            }
        }
    }
    // The arm's own fingerprint: the per-step injection ran (the exact T⁴
    // radiosity's activity is strictly positive over a year that swings
    // surface temperatures).
    assert!(
        arm.interior_lwr_activity_kwh > 0.0,
        "case {}: the ScriptF arm's interior-LWR injection activity must be \
         positive (the selector's fingerprint)",
        case_id
    );

    // The arm's energy closure on the free-float cases stays inside the
    // documented reconciliation envelope the 600FF residual test pins
    // (max 1500 W, mean 600 W): the post-processed accounts ride runs
    // that close to the same tolerance the production runs close to.
    if matches!(case_id, "600FF" | "900FF") {
        for (label, record) in [("control", &control), ("arm", &arm)] {
            let mean = record.closure_residual_sum_abs_w / record.steps as f64;
            assert!(
                record.closure_residual_max_abs_w < 1500.0 && mean < 600.0,
                "case {}: the {} run's zone-air balance residual (max {:.3} W, \
                 mean {:.3} W) leaves the documented closure envelope",
                case_id,
                label,
                record.closure_residual_max_abs_w,
                mean
            );
        }
    }

    print_arm_delta(&control, &arm);

    // Arm d's identity verdict: the driven series against the EPW's own
    // monthly ground row and the case definition's constant. The verdict
    // is the record; the direction statement (whether aligning the input
    // could close the under-heating residuals) is data for the report,
    // not an assertion.
    let epw_ground = epw_monthly_ground_c();
    let driven = &control.ground_monthly_mean_c;
    let max_driven_epw = driven
        .iter()
        .zip(epw_ground.iter())
        .map(|(d, e)| (d - e).abs())
        .fold(0.0_f64, f64::max);
    let annual_mean_driven = driven.iter().sum::<f64>() / 12.0;
    eprintln!(
        "[arm-d] case={} ground identity: driven annual mean={:.3} C vs the case \
         definition's constant {:.1} C; max monthly |driven - EPW row|={:.3} K \
         (driven Jan/Feb [{:.3}, {:.3}], EPW Jan/Feb [{:.2}, {:.2}], constant \
         {:.1}): the driven series is the dry-bulb-derived Kusuda-Achenbach \
         surface curve, not the EPW's 0.5 m row and not the constant; the \
         winter months sit below the constant, the heating-adding direction",
        case_id,
        annual_mean_driven,
        BESTEST_CONSTANT_GROUND_C,
        max_driven_epw,
        driven[0],
        driven[1],
        epw_ground[0],
        epw_ground[1],
        BESTEST_CONSTANT_GROUND_C,
    );
}

/// The per-case arm pairs. Each test runs two annual simulations (the
/// control and the ScriptF arm) and prints the causal table's row; the
/// control's bitwise pin and the selector fingerprints are the
/// assertions, the deltas are the measurement.
mod arms {
    use super::ARM_CASE_IDS;
    use super::run_case_arms;

    macro_rules! case_arms {
        ($id:literal, $name:ident) => {
            #[test]
            fn $name() {
                run_case_arms($id);
            }
        };
    }

    case_arms!("600", arms_case_600);
    case_arms!("900", arms_case_900);
    case_arms!("600FF", arms_case_600ff);
    case_arms!("900FF", arms_case_900ff);
    case_arms!("195", arms_case_195);
    case_arms!("395", arms_case_395);

    /// The case registry is exactly the study's six.
    #[test]
    fn arm_registry_is_exactly_the_study_cases() {
        let registry: Vec<String> = super::core_cases()
            .into_iter()
            .chain(super::in_depth_cases())
            .map(|c| c.id.to_string())
            .collect();
        for id in ARM_CASE_IDS {
            assert!(
                registry.iter().any(|r| r == id),
                "case {id} missing from the case registry"
            );
        }
        assert_eq!(ARM_CASE_IDS.len(), 6, "the study measures six cases");
    }
}
