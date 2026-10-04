//! Manifest acceptance tests: unknown keys are rejected, dwelling manifests
//! need exactly one home, fleet manifests carry a weight on every home and
//! a resolution while dwelling manifests reject both keys, a manifest's
//! `[tariff]` table is dwelling-only with a parseable zone and a loadable
//! tariff file, and compare-all selects manifests by exact feature set.

use std::collections::BTreeMap;

use frame_golden::manifest::{
    GoldenManifest, ManifestResolution, running_features, select_manifests,
};

/// The flat tariff the golden fixtures point at, as JSON, for the
/// `[tariff]` file tests to corrupt.
const FLAT_TARIFF: &str = r#"{
    "name": "flat",
    "tou_schedule": [
        {
            "name": "flat",
            "schedule": [
                {"day": "Any", "start_minute": 0, "end_minute": 1440, "value": 0.0}
            ],
            "season": "All"
        }
    ],
    "demand_tou_schedule": [],
    "energy_rates": [
        {"period_name": "flat", "season": "All", "rate_per_kwh": 0.15}
    ],
    "demand_rates": [],
    "tiered_rates": [],
    "export_rate": {"mode": "None", "tou_credits": []},
    "fixed_charges": {"monthly_usd": 10.0, "daily_usd": 0.0},
    "billing_cycle": "Monthly"
}"#;

fn manifest_text(kind: &str, features: &str, homes: usize) -> String {
    let mut text = String::new();
    text.push_str(&format!("kind = \"{kind}\"\n"));
    text.push_str(&format!("features = {features}\n"));
    text.push_str("defaults = \"defaults\"\n");
    text.push_str("\n[simulation]\n");
    text.push_str("start_time = \"2023-01-01T00:00:00-07:00\"\n");
    text.push_str("duration = 86400\n");
    text.push_str("time_res = 3600\n");
    text.push_str("output_verbosity = 2\n");
    text.push_str("master_seed = 0\n");
    for bldg_id in 1..=homes {
        text.push_str(&format!(
            "\n[[home]]\nbldg_id = {bldg_id}\nhpxml = \"building.xml\"\nschedule = \"schedule.csv\"\nweather = \"weather.epw\"\ninitialization_duration_s = 0\noverrides = {{}}\n"
        ));
    }
    text
}

#[test]
fn manifest_rejects_unknown_key() {
    let base = manifest_text("dwelling", "[]", 1);

    let top_level = base.replace(
        "defaults = \"defaults\"",
        "defaults = \"defaults\"\nunknown_key = 1",
    );
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &top_level)
        .expect_err("an unknown top-level key must be rejected");
    assert!(
        error.to_string().contains("unknown_key"),
        "the error names the key: {error}"
    );

    let simulation = base.replace("[simulation]", "[simulation]\nunknown_sim_key = 1");
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &simulation)
        .expect_err("an unknown [simulation] key must be rejected");
    assert!(
        error.to_string().contains("unknown_sim_key"),
        "the error names the key: {error}"
    );

    let home = base.replace("overrides = {}", "overrides = {}\nunknown_home_key = 1");
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &home)
        .expect_err("an unknown [[home]] key must be rejected");
    assert!(
        error.to_string().contains("unknown_home_key"),
        "the error names the key: {error}"
    );
}

#[test]
fn manifest_dwelling_kind_requires_exactly_one_home() {
    let two_homes = manifest_text("dwelling", "[]", 2);
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &two_homes)
        .expect_err("a dwelling manifest with two homes must be rejected");
    assert!(
        error.to_string().contains("exactly one"),
        "the error states the rule: {error}"
    );

    let zero_homes = manifest_text("dwelling", "[]", 0);
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &zero_homes)
        .expect_err("a dwelling manifest with no home must be rejected");
    assert!(error.to_string().contains("exactly one"));

    let one_home = manifest_text("dwelling", "[]", 1);
    assert!(GoldenManifest::parse(std::path::Path::new("fixture.toml"), &one_home).is_ok());

    let fleet_one = manifest_text("fleet", "[]", 1);
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &fleet_one)
        .expect_err("a fleet manifest with one home must be rejected");
    assert!(
        error.to_string().contains("two or more"),
        "the error states the rule: {error}"
    );

    let fleet_two = manifest_text("fleet", "[]", 2)
        .replace("overrides = {}", "weight = 1.0\noverrides = {}")
        .replace(
            "defaults = \"defaults\"",
            "defaults = \"defaults\"\nresolution = \"hourly\"",
        );
    let manifest = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &fleet_two).unwrap();
    assert_eq!(manifest.home.len(), 2);
}

#[test]
fn manifest_fleet_requires_weight_and_resolution() {
    // A resolution but no weight on any home: the error names the missing
    // field.
    let missing_weight = manifest_text("fleet", "[]", 2).replace(
        "defaults = \"defaults\"",
        "defaults = \"defaults\"\nresolution = \"hourly\"",
    );
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &missing_weight)
        .expect_err("a fleet home without a weight must be rejected");
    assert!(
        error.to_string().contains("weight"),
        "the error names the field: {error}"
    );

    // Weights without a resolution: the error names the missing field.
    let missing_resolution =
        manifest_text("fleet", "[]", 2).replace("overrides = {}", "weight = 1.0\noverrides = {}");
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &missing_resolution)
        .expect_err("a fleet manifest without a resolution must be rejected");
    assert!(
        error.to_string().contains("resolution"),
        "the error names the field: {error}"
    );

    // Both keys present: the manifest parses and keeps the resolution.
    let full = missing_weight.replace("overrides = {}", "weight = 1.0\noverrides = {}");
    let manifest = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &full).unwrap();
    assert_eq!(manifest.resolution, Some(ManifestResolution::Hourly));

    // The fifteen-minute spelling round-trips too.
    let fifteen = full.replace("resolution = \"hourly\"", "resolution = \"fifteen_min\"");
    let manifest = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &fifteen).unwrap();
    assert_eq!(manifest.resolution, Some(ManifestResolution::FifteenMin));

    // A resolution string outside the enum is rejected.
    let unknown = full.replace("resolution = \"hourly\"", "resolution = \"monthly\"");
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &unknown)
        .expect_err("an unknown resolution string must be rejected");
    assert!(
        error.to_string().contains("monthly"),
        "the error names the unknown string: {error}"
    );
}

#[test]
fn manifest_dwelling_rejects_fleet_keys() {
    let weight = manifest_text("dwelling", "[]", 1)
        .replace("overrides = {}", "weight = 1.0\noverrides = {}");
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &weight)
        .expect_err("a dwelling home carrying a weight must be rejected");
    let error = error.to_string();
    assert!(
        error.contains("weight"),
        "the error names the field: {error}"
    );
    assert!(
        error.contains("fleet"),
        "the error states the kind rule: {error}"
    );

    let resolution = manifest_text("dwelling", "[]", 1).replace(
        "defaults = \"defaults\"",
        "defaults = \"defaults\"\nresolution = \"hourly\"",
    );
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &resolution)
        .expect_err("a dwelling manifest carrying a resolution must be rejected");
    let error = error.to_string();
    assert!(
        error.contains("resolution"),
        "the error names the field: {error}"
    );
    assert!(
        error.contains("fleet"),
        "the error states the kind rule: {error}"
    );
}

#[test]
fn manifest_tariff_rejected_for_fleet() {
    let fleet = format!(
        "{}\n[tariff]\nfile = \"tariff.json\"\nzone = \"America/Denver\"\n",
        manifest_text("fleet", "[]", 2)
    );
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &fleet)
        .expect_err("a fleet manifest with a [tariff] table must be rejected");
    assert!(
        error.to_string().contains("dwelling"),
        "the error states the rule: {error}"
    );
}

#[test]
fn manifest_tariff_rejects_unknown_zone() {
    let base = manifest_text("dwelling", "[]", 1);
    let manifest = format!("{base}\n[tariff]\nfile = \"tariff.json\"\nzone = \"Not/AZone\"\n");
    let error = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &manifest)
        .expect_err("an unparseable IANA zone must be rejected");
    assert!(
        error.to_string().contains("Not/AZone"),
        "the error names the zone: {error}"
    );
}

/// A `[tariff]` file that cannot be parsed, or that fails `validate()`,
/// fails `GoldenManifest::load` naming the manifest, the `file` field and
/// the cause. The `file` value is the absolute path of a tariff file
/// written beside the manifest, which `repo_path` returns as given, so no
/// test file lands in the tree.
#[test]
fn manifest_tariff_rejects_invalid_file() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("fixture.toml");
    let base = manifest_text("dwelling", "[]", 1);

    // A tariff JSON with a misspelled key.
    let mut tariff: serde_json::Value = serde_json::from_str(FLAT_TARIFF).unwrap();
    tariff["misspelled_key"] = serde_json::json!(1);
    let misspelled_path = dir.path().join("misspelled.json");
    std::fs::write(&misspelled_path, tariff.to_string()).unwrap();
    let manifest = format!(
        "{base}\n[tariff]\nfile = \"{}\"\nzone = \"America/Denver\"\n",
        misspelled_path.display()
    );
    std::fs::write(&manifest_path, &manifest).unwrap();
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a tariff file with an unknown key must fail the load");
    let error = error.to_string();
    assert!(
        error.contains("fixture.toml"),
        "the error names the manifest: {error}"
    );
    assert!(error.contains("file"), "the error names the field: {error}");
    assert!(
        error.contains("misspelled_key"),
        "the error names the cause: {error}"
    );

    // A tariff file that is not JSON.
    let not_json_path = dir.path().join("not_json.json");
    std::fs::write(&not_json_path, "this is not json").unwrap();
    let manifest = format!(
        "{base}\n[tariff]\nfile = \"{}\"\nzone = \"America/Denver\"\n",
        not_json_path.display()
    );
    std::fs::write(&manifest_path, &manifest).unwrap();
    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a tariff file that is not JSON must fail the load");
    let error = error.to_string();
    assert!(
        error.contains("fixture.toml"),
        "the error names the manifest: {error}"
    );
    assert!(error.contains("file"), "the error names the field: {error}");
    assert!(
        error.contains("expected"),
        "the error names the cause: {error}"
    );
}

#[test]
fn manifest_tariff_rejects_failed_validation() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("fixture.toml");
    let mut tariff: serde_json::Value = serde_json::from_str(FLAT_TARIFF).unwrap();
    tariff["demand_window_minutes"] = serde_json::json!(2);
    let tariff_path = dir.path().join("tariff.json");
    std::fs::write(&tariff_path, tariff.to_string()).unwrap();
    let base = manifest_text("dwelling", "[]", 1);
    let manifest = format!(
        "{base}\n[tariff]\nfile = \"{}\"\nzone = \"America/Denver\"\n",
        tariff_path.display()
    );
    std::fs::write(&manifest_path, &manifest).unwrap();

    let error = GoldenManifest::load(&manifest_path)
        .expect_err("a tariff failing validate() must fail the load");
    let error = error.to_string();
    assert!(
        error.contains("fixture.toml"),
        "the error names the manifest: {error}"
    );
    assert!(error.contains("file"), "the error names the field: {error}");
    assert!(
        error.contains("demand_window_minutes"),
        "the error names validate()'s error: {error}"
    );
}

#[test]
fn compare_all_selects_manifests_by_exact_feature_set() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plain.toml"),
        manifest_text("dwelling", "[]", 1),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("dst.toml"),
        manifest_text("dwelling", "[\"dst\"]", 1),
    )
    .unwrap();

    let running = running_features();
    let selected: BTreeMap<String, ()> = select_manifests(dir.path(), &running)
        .unwrap()
        .into_iter()
        .map(|path| (path.file_name().unwrap().to_string_lossy().to_string(), ()))
        .collect();

    let expected = if running.iter().any(|feature| feature == "dst") {
        BTreeMap::from([("dst.toml".to_string(), ())])
    } else {
        BTreeMap::from([("plain.toml".to_string(), ())])
    };
    assert_eq!(
        selected, expected,
        "compare-all must select exactly the manifests whose feature set equals the running binary's"
    );
}
