use std::fs;
use std::path::{Path, PathBuf};

fn collect_rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("read_dir");
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_new_imperial_conversion_markers_in_equipment_src() {
    // Guard intent:
    // - conversions between imperial HVAC efficiency units and SI must live in IO/periphery,
    //   not in equipment simulation code.
    // - this test allows a narrow baseline list while preventing any new spread.
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_root, &mut files);

    // Substring marker -> allowlist of source-relative files that may contain it.
    // Keep this list intentionally tiny; remove entries as hard-cut migration lands.
    let rules: &[(&str, &[&str])] = &[
        (
            "BTU_PER_HR_PER_W",
            &[
                "hvac/air_conditioner.rs",
                "hvac/heat_pump_config.rs",
                "hvac/heat_pump/heater_config.rs",
            ],
        ),
        ("HeatingEfficiencyUnit::Seer", &["hvac/heat_pump_config.rs"]),
        ("HeatingEfficiencyUnit::Eer", &["hvac/heat_pump_config.rs"]),
        ("HeatingEfficiencyUnit::Hspf", &["hvac/heat_pump_config.rs"]),
    ];

    let mut violations = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&src_root)
            .expect("strip prefix")
            .to_string_lossy()
            .replace('\\', "/");
        let content = fs::read_to_string(&file).expect("read source file");

        for (marker, allowed) in rules {
            if content.contains(marker) && !allowed.iter().any(|p| *p == rel) {
                violations.push(format!(
                    "forbidden imperial marker '{marker}' found in {rel}"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "SI guard failed:\n{}",
        violations.join("\n")
    );
}
