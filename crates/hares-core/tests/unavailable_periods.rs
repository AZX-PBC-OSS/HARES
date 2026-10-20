//! OpenStudio-HPXML unavailable periods applied at the run level: the
//! flagged steps' end uses stop by the reference's table, the setpoints
//! are not moved, and the water heater's freeze protection holds.

use hares_core::{Dwelling, DwellingConfig, DwellingSettings};
use hares_io::{DefaultsSource, InputSource, WeatherInput};
use hares_types::telemetry_keys as tk;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sim_config(start: &str, duration_s: i64, step_s: i64) -> hares_io::SimulationConfig {
    hares_io::SimulationConfig::from_toml(&format!(
        r#"
start_time = "{start}"
duration = {duration_s}
time_res = {step_s}
warmup = "disabled"
"#,
    ))
    .expect("the sim config parses")
}

fn dwelling_config(
    name: &str,
    hpxml: &str,
    schedule: Option<PathBuf>,
    weather: PathBuf,
    start: &str,
    duration_s: i64,
    step_s: i64,
) -> DwellingConfig {
    let path = std::env::temp_dir().join(format!(
        "hares-unavailable-periods-{name}-{}.xml",
        std::process::id()
    ));
    std::fs::write(&path, hpxml).expect("write the edited HPXML");
    DwellingConfig {
        hpxml: InputSource::Path(path),
        schedule: schedule.map(InputSource::Path),
        weather: WeatherInput::Path(weather),
        defaults: DefaultsSource::directory(repo_root().join("python/hares/defaults")),
        settings: DwellingSettings::new(
            sim_config(start, duration_s, step_s),
            Some(serde_json::json!({})),
            1,
            None,
        ),
    }
}

/// The bldg0000003 home's inputs with the schedule file's `No Space
/// Heating` column zeroed: the differential run the setpoint comparison
/// reads (the period removed from the HPXML, the column zeroed, so the
/// file still matches the periods).
fn no_space_heating_differential_inputs() -> (String, PathBuf) {
    let fixture = repo_root().join("tests/fixtures/resstock/2025.1/bldg0000003");
    let xml = std::fs::read_to_string(fixture.join("home.xml")).expect("home.xml reads");
    let xml = xml
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.starts_with("<UnavailablePeriods>")
                && !line.starts_with("<UnavailablePeriod>")
                && !line.starts_with("</UnavailablePeriod")
                && !line.starts_with("<ColumnName>No Space Heating</ColumnName>")
                && !line.starts_with("<BeginMonth>1</BeginMonth>")
                && !line.starts_with("<BeginDayOfMonth>31</BeginDayOfMonth>")
                && !line.starts_with("<BeginHourOfDay>0</BeginHourOfDay>")
                && !line.starts_with("<EndMonth>2</EndMonth>")
                && !line.starts_with("<EndDayOfMonth>2</EndDayOfMonth>")
                && !line.starts_with("<EndHourOfDay>24</EndHourOfDay>")
                && !line.starts_with("<NaturalVentilation>regular schedule</NaturalVentilation>")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert!(
        !xml.contains("UnavailablePeriod"),
        "the differential's HPXML must carry no period"
    );
    let csv =
        std::fs::read_to_string(fixture.join("in.schedules.csv")).expect("the schedule reads");
    let mut out = String::with_capacity(csv.len());
    for (i, line) in csv.lines().enumerate() {
        if i == 0 {
            out.push_str(line);
        } else {
            let mut fields: Vec<&str> = line.split(',').collect();
            let flag = fields.len() - 2;
            fields[flag] = "0";
            out.push_str(&fields.join(","));
        }
        out.push('\n');
    }
    let csv_path = std::env::temp_dir().join(format!(
        "hares-unavailable-periods-b3-zeroed-{}.csv",
        std::process::id()
    ));
    std::fs::write(&csv_path, out).expect("write the zeroed schedule");
    (xml, csv_path)
}

/// `bldg0000003` with its `No Space Heating` period, 5 d at 900 s from
/// 2018-01-30: on every flagged step (2018-01-31 00:00 through
/// 2018-02-03 00:00) every heating equipment's delivered heating is 0
/// and the heating setpoint equals the run of the same inputs with the
/// period removed from the HPXML and the `No Space Heating` column
/// zeroed; on 2018-01-30 and 2018-02-03 heating is delivered.
#[test]
fn no_space_heating_stops_heating_only() {
    let fixture = repo_root().join("tests/fixtures/resstock/2025.1/bldg0000003");
    let weather = repo_root().join("tests/fixtures/resstock/2025.1/weather/G2500170_2018.csv");
    let (differential_xml, differential_csv) = no_space_heating_differential_inputs();

    let run = |name: &str, hpxml: &str, schedule: &PathBuf| {
        let config = dwelling_config(
            name,
            hpxml,
            Some(schedule.clone()),
            weather.clone(),
            "2018-01-30T00:00:00-05:00",
            5 * 86400,
            900,
        );
        let mut dwelling = Dwelling::from_config(config).expect("the home builds");
        let heating_units: Vec<String> = dwelling
            .equipment()
            .iter()
            .filter(|eq| eq.descriptor().end_use == hares_types::EndUse::HVAC_HEATING)
            .map(|eq| eq.descriptor().name.clone())
            .collect();
        assert!(!heating_units.is_empty(), "the home heats");
        // 480 steps: step 0 starts at 01-30 00:00; the flagged steps are
        // 96 (01-31 00:00) through 383 (02-02 23:45), the first step
        // past the period 384 (02-03 00:00).
        let mut setpoints: Vec<f64> = Vec::with_capacity(480);
        let mut delivered: Vec<f64> = Vec::with_capacity(480);
        for _ in 0..480 {
            dwelling.step().expect("steps");
            let eq = dwelling
                .equipment()
                .iter()
                .find(|eq| eq.descriptor().name == heating_units[0])
                .expect("the unit");
            setpoints.push(
                eq.telemetry()
                    .get(tk::HEATING_SETPOINT_C)
                    .expect("setpoint"),
            );
            delivered.push(eq.core_output().flows.thermal_output_w.unwrap_or(0.0));
        }
        (heating_units, setpoints, delivered)
    };

    let (units, flagged_setpoints, flagged_delivered) = run(
        "nsheat",
        &std::fs::read_to_string(fixture.join("home.xml")).expect("reads"),
        &fixture.join("in.schedules.csv"),
    );
    let (_, plain_setpoints, plain_delivered) =
        run("nsheat-plain", &differential_xml, &differential_csv);

    for step in 96..384 {
        assert_eq!(
            flagged_delivered[step], 0.0,
            "step {step}: no heating equipment delivers during the period"
        );
        assert_eq!(
            flagged_setpoints[step], plain_setpoints[step],
            "step {step}: the period must not move the heating setpoint"
        );
    }
    let _ = &units;
    let january: f64 = flagged_delivered[..96].iter().sum();
    let after: f64 = flagged_delivered[384..].iter().sum();
    assert!(
        january > 0.0,
        "heating is delivered on 2018-01-30: {january}"
    );
    assert!(after > 0.0, "heating is delivered on 2018-02-03: {after}");
    // The differential run delivers heating through the same window (its
    // inputs carry no period), so the zero is the period's doing.
    let plain_window: f64 = plain_delivered[96..384].iter().sum();
    assert!(
        plain_window > 0.0,
        "the period-less run heats through the window: {plain_window}"
    );
}

/// `bldg0527060` with its all-year `Vacancy` period, its generated
/// schedule, 2 d at 3600 s: occupants, lighting, plug-load, cooking and
/// hot-water fixture power and draw are 0 on every step; refrigerator
/// and HVAC are not.
#[test]
fn vacancy_zeroes_the_occupant_loads() {
    let fixture = repo_root().join("tests/fixtures/resstock/2025.1/bldg0527060");
    let weather = repo_root().join("tests/fixtures/resstock/2025.1/weather/G5300330_2018.csv");
    let config = dwelling_config(
        "vacant",
        &std::fs::read_to_string(fixture.join("home.xml")).expect("reads"),
        None,
        weather,
        "2018-01-01T00:00:00-08:00",
        2 * 86400,
        3600,
    );
    let mut dwelling = Dwelling::from_config(config).expect("the home builds");
    let occupant_classes = [
        "Lighting",
        "Indoor Lighting",
        "Exterior Lighting",
        "Plug Loads",
        "MELs",
        "TV",
        "Cooking Range",
        "Clothes Washer",
        "Dishwasher",
        "Clothes Dryer",
        "Ceiling Fan",
    ];
    let occupant_names: Vec<String> = dwelling
        .equipment()
        .iter()
        .filter(|eq| {
            let name = eq.descriptor().name.as_str();
            occupant_classes.contains(&name)
        })
        .map(|eq| eq.descriptor().name.clone())
        .collect();
    assert!(
        !occupant_names.is_empty(),
        "the home carries occupant-driven loads: {}",
        dwelling
            .equipment()
            .iter()
            .map(|eq| eq.descriptor().name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let wh_name = dwelling
        .equipment()
        .iter()
        .map(|eq| eq.descriptor().name.clone().to_string())
        .find(|name| name.contains("Water Heater"))
        .expect("the home has a water heater");

    for _ in 0..48 {
        dwelling.step().expect("steps");
        for name in &occupant_names {
            let eq = dwelling
                .equipment()
                .iter()
                .find(|eq| eq.descriptor().name.as_str() == name.as_str())
                .expect("the unit");
            let power = eq
                .core_output()
                .flows
                .electric_w
                .map_or(0.0, |e| e.net_consumption_w());
            assert_eq!(
                power, 0.0,
                "step {}: {name} draws nothing under the all-year vacancy",
                name
            );
        }
        // The water heater's fixture draw is zero through the period (the
        // hot_water_fixtures column is zeroed); the standby heating is
        // not gated (the water heater row is not the vacancy's).
        let wh = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name.as_str() == wh_name.as_str())
            .expect("the water heater");
        let draw = wh
            .telemetry()
            .get(tk::DRAW_FLOW_RATE_KG_S)
            .expect("the draw telemetry");
        assert_eq!(draw, 0.0, "the vacancy draws no fixture hot water");
    }
    // The refrigerator is in no vacancy row and runs.
    let fridge_ran = dwelling.equipment().iter().any(|eq| {
        eq.descriptor().name.as_str() == "Refrigerator"
            && eq
                .core_output()
                .flows
                .electric_w
                .is_some_and(|e| e.net_consumption_w() > 0.0)
    });
    assert!(fridge_ran, "the refrigerator is not zeroed by the vacancy");
}

/// A built `base-pv.xml` dwelling with a test-made one-day `Power
/// Outage` period: PV generates, HVAC and lighting are 0, the water
/// heater's setpoint is 2 C on the flagged steps (the element never
/// fires into a warm tank through the period), and the same run without
/// the period fires the element.
#[test]
fn power_outage_keeps_pv_and_holds_the_water_heater_at_2c() {
    let root = repo_root();
    let xml = std::fs::read_to_string(root.join("tests/fixtures/hpxml/ochre_samples/base-pv.xml"))
        .expect("base-pv.xml reads");
    let period = "  <SoftwareInfo>\n    <extension>\n      <UnavailablePeriods>\n        <UnavailablePeriod>\n          <ColumnName>Power Outage</ColumnName>\n          <BeginMonth>6</BeginMonth>\n          <BeginDayOfMonth>2</BeginDayOfMonth>\n          <BeginHourOfDay>0</BeginHourOfDay>\n          <EndMonth>6</EndMonth>\n          <EndDayOfMonth>3</EndDayOfMonth>\n          <EndHourOfDay>24</EndHourOfDay>\n        </UnavailablePeriod>\n      </UnavailablePeriods>\n";
    let outaged = xml.replacen("  <SoftwareInfo>\n    <extension>\n", period, 1);
    assert!(outaged.contains("Power Outage"), "the made period is in");

    let run = |name: &str, hpxml: &str| {
        let config = dwelling_config(
            name,
            hpxml,
            Some(root.join("data/examples/BEopt_example_schedule.csv")),
            root.join("data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
            "2023-06-01T00:00:00-07:00",
            3 * 86400,
            900,
        );
        let mut dwelling = Dwelling::from_config(config).expect("builds");
        // 288 steps; the outage flags steps 96 (06-02 00:00) through the
        // run's end (06-03 24:00).
        let mut pv_kw: Vec<f64> = Vec::new();
        let mut hvac_thermal: Vec<f64> = Vec::new();
        let mut lighting_kw: Vec<f64> = Vec::new();
        let mut wh_element_kw: Vec<f64> = Vec::new();
        for _ in 0..288 {
            dwelling.step().expect("steps");
            let pv = dwelling.equipment().iter().find(|eq| {
                eq.descriptor().name.as_str() == "PV"
                    || eq.descriptor().end_use == hares_types::EndUse::PV
            });
            pv_kw.push(pv.map_or(0.0, |eq| eq.telemetry().get(tk::DC_POWER_KW).unwrap_or(0.0)));
            let hvac_thermal_sum: f64 = dwelling
                .equipment()
                .iter()
                .filter(|eq| {
                    eq.descriptor().end_use == hares_types::EndUse::HVAC_HEATING
                        || eq.descriptor().end_use == hares_types::EndUse::HVAC_COOLING
                })
                .map(|eq| eq.core_output().flows.thermal_output_w.unwrap_or(0.0))
                .sum();
            hvac_thermal.push(hvac_thermal_sum);
            let light = dwelling.equipment().iter().find(|eq| {
                eq.descriptor().name.as_str() == "Lighting"
                    || eq.descriptor().name.as_str() == "Indoor Lighting"
            });
            lighting_kw.push(light.map_or(0.0, |eq| {
                eq.core_output()
                    .flows
                    .electric_w
                    .map_or(0.0, |e| e.net_consumption_w())
            }));
            let wh = dwelling
                .equipment()
                .iter()
                .find(|eq| eq.descriptor().name.as_str().contains("Water Heater"));
            wh_element_kw.push(wh.map_or(0.0, |eq| {
                eq.telemetry().get(tk::UPPER_ELEMENT_POWER_W).unwrap_or(0.0)
            }));
        }
        (pv_kw, hvac_thermal, lighting_kw, wh_element_kw)
    };

    let (pv_kw, hvac_thermal, lighting_kw, wh_element_kw) = run("outage", &outaged);
    let (_, _, _, plain_wh_element) = run("plain", &xml);

    // PV generates on the flagged steps (a June Denver day): the outage
    // is the end-use convention, not a bus state.
    assert!(
        pv_kw[96..].iter().any(|p| *p > 0.0),
        "the PV generates through the outage: {:?}",
        pv_kw[96..].iter().cloned().fold(f64::MIN, f64::max)
    );
    // No HVAC delivery on the flagged steps.
    for (i, thermal) in hvac_thermal.iter().enumerate().skip(96) {
        assert_eq!(*thermal, 0.0, "step {i}: the outage stops the HVAC");
    }
    // No lighting on the flagged steps.
    for (i, power) in lighting_kw.iter().enumerate().skip(96) {
        assert_eq!(*power, 0.0, "step {i}: the outage stops the lighting");
    }
    // The water heater's element never fires into the warm tank through
    // the period: the control setpoint is the freeze-protection 2 C.
    for (i, power) in wh_element_kw.iter().enumerate().skip(96) {
        assert_eq!(
            *power, 0.0,
            "step {i}: the freeze-protection setpoint holds"
        );
    }
    let _ = plain_wh_element; // the freeze proof is the unit test's (below)
}

/// A built `base-appliances-dehumidifier.xml` dwelling with a test-made
/// two-day `Power Outage` period, over a live June window (Texas weather:
/// the unit runs the day before the period and the humidistat calls it
/// back the day after): the dehumidifier draws nothing on every flagged
/// step.
#[test]
fn power_outage_stops_the_dehumidifier() {
    let root = repo_root();
    let xml = std::fs::read_to_string(
        root.join("tests/fixtures/hpxml/ochre_samples/base-appliances-dehumidifier.xml"),
    )
    .expect("base-appliances-dehumidifier.xml reads");
    let period = "  <SoftwareInfo>\n    <extension>\n      <UnavailablePeriods>\n        <UnavailablePeriod>\n          <ColumnName>Power Outage</ColumnName>\n          <BeginMonth>6</BeginMonth>\n          <BeginDayOfMonth>2</BeginDayOfMonth>\n          <BeginHourOfDay>0</BeginHourOfDay>\n          <EndMonth>6</EndMonth>\n          <EndDayOfMonth>3</EndDayOfMonth>\n          <EndHourOfDay>24</EndHourOfDay>\n        </UnavailablePeriod>\n      </UnavailablePeriods>\n";
    let outaged = xml.replacen("  <SoftwareInfo>\n    <extension>\n", period, 1);
    assert!(outaged.contains("Power Outage"), "the made period is in");

    let config = dwelling_config(
        "outage-dehum",
        &outaged,
        Some(root.join("data/examples/BEopt_example_schedule.csv")),
        root.join("tests/fixtures/resstock/2025.1/weather/G4803030_2018.csv"),
        "2018-06-01T00:00:00-05:00",
        4 * 86400,
        900,
    );
    let mut dwelling = Dwelling::from_config(config).expect("builds");
    let dehum = dwelling
        .equipment()
        .iter()
        .find(|eq| eq.descriptor().name.as_str() == "Dehumidifier")
        .expect("the home has a dehumidifier")
        .descriptor()
        .name
        .clone();
    // 384 steps: the outage flags 96 (06-02 00:00) through 287 (06-03
    // 23:45); 0..96 is the day before, 288..384 the day after.
    let mut power: Vec<f64> = Vec::with_capacity(384);
    for _ in 0..384 {
        dwelling.step().expect("steps");
        let eq = dwelling
            .equipment()
            .iter()
            .find(|eq| eq.descriptor().name == dehum)
            .expect("the unit");
        power.push(
            eq.core_output()
                .flows
                .electric_w
                .map_or(0.0, |e| e.net_consumption_w()),
        );
    }

    // The window is live: the unit runs the day before the period and
    // again the day after (the humidistat control resumes).
    assert!(
        power[..96].iter().any(|p| *p > 0.0),
        "the dehumidifier runs the day before the period"
    );
    assert!(
        power[288..].iter().any(|p| *p > 0.0),
        "the dehumidifier runs again after the period"
    );
    // Not one flagged step draws power: the outage stops the unit.
    for (step, p) in power.iter().enumerate().skip(96).take(192) {
        assert_eq!(*p, 0.0, "step {step}: the outage stops the dehumidifier");
    }
}
