//! Defaults-tree acceptance tests: a `[defaults_files]` replacement
//! materializes into the prepared tree and never touches the repository
//! file the prepared tree links to.

use frame_golden::defaults;
use frame_golden::manifest::{GoldenManifest, repo_root};

/// A manifest replacing one defaults file with another repository file.
/// The replacement source must exist in-repo (replacements resolve
/// repository-relative), and the two files must differ for the test to
/// mean anything.
fn replacement_manifest() -> String {
    r#"
kind = "dwelling"
features = []
defaults = "defaults"

[defaults_files]
"pv/premium_470.toml" = "defaults/generator/efficiency_curve.toml"

[simulation]
start_time = "2023-01-01T00:00:00-07:00"
duration = 86400
time_res = 3600
output_verbosity = 2
master_seed = 0

[[home]]
bldg_id = 1
hpxml = "tests/fixtures/parity/cz4a_ashp_hpwh/building.xml"
schedule = "tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv"
weather = "tests/fixtures/parity/cz4a_ashp_hpwh/weather.epw"
initialization_duration_s = 0
overrides = {}
"#
    .to_string()
}

#[test]
fn defaults_replacement_leaves_the_repo_file_untouched() {
    let manifest = GoldenManifest::parse(
        std::path::Path::new("fixture.toml"),
        &replacement_manifest(),
    )
    .unwrap();
    let root = repo_root(std::path::Path::new("fixture.toml"));

    let repo_file = root.join("defaults/pv/premium_470.toml");
    let replacement = std::fs::read(root.join("defaults/generator/efficiency_curve.toml")).unwrap();
    let before = std::fs::read(&repo_file).unwrap();
    assert_ne!(
        before, replacement,
        "the test needs a replacement distinct from the file it replaces"
    );

    let tree = defaults::prepare(&root, &manifest).unwrap();
    let prepared = std::fs::read(tree.dir().join("pv/premium_470.toml")).unwrap();
    assert_eq!(
        prepared, replacement,
        "the prepared tree must carry the replacement bytes"
    );

    let after = std::fs::read(&repo_file).unwrap();
    assert_eq!(
        before, after,
        "the replacement must not overwrite the repository's own file"
    );
}
