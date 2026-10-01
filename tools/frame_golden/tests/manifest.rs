//! Manifest acceptance tests: unknown keys are rejected, dwelling manifests
//! need exactly one home, and compare-all selects manifests by exact
//! feature set.

use std::collections::BTreeMap;

use frame_golden::manifest::{GoldenManifest, running_features, select_manifests};

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

    let fleet_two = manifest_text("fleet", "[]", 2);
    let manifest = GoldenManifest::parse(std::path::Path::new("fixture.toml"), &fleet_two).unwrap();
    assert_eq!(manifest.home.len(), 2);
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
