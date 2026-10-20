//! The wheel's packaging: the defaults tree ships inside the package at
//! `hares/defaults/`, with no top-level `defaults/` entry, so a caller
//! without an explicit defaults path resolves the tree from the installed
//! package's own location. The test runs a release wheel build, so it is
//! ignored by default and run explicitly when the packaging changes.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "runs a release wheel build; run with --ignored when the packaging changes"]
fn wheel_carries_defaults_inside_the_package() {
    let root = repo_root();
    let out_dir = tempfile::tempdir().expect("create temp dir");
    let status = Command::new("uv")
        .args(["run", "--no-sync", "maturin", "build", "--release"])
        .arg("--out")
        .arg(out_dir.path())
        .env("CARGO_BUILD_JOBS", "2")
        .current_dir(&root)
        .status()
        .expect("spawn maturin build");
    assert!(status.success(), "the wheel build succeeded");

    let mut wheels: Vec<PathBuf> = std::fs::read_dir(out_dir.path())
        .expect("read the output directory")
        .filter_map(|entry| {
            let path = entry.expect("entry").path();
            (path.extension().is_some_and(|ext| ext == "whl")).then_some(path)
        })
        .collect();
    assert_eq!(wheels.len(), 1, "exactly one wheel built");
    let wheel = wheels.pop().expect("one wheel");

    // The wheel's file list, read by the environment's Python: the zip
    // listing needs no third-party crate on the Rust side.
    let python = root.join(".venv/bin/python");
    let script = "import json, sys, zipfile; wheel = zipfile.ZipFile(sys.argv[1]); \
                  print(json.dumps(wheel.namelist()))";
    let output = Command::new(&python)
        .arg("-c")
        .arg(script)
        .arg(&wheel)
        .output()
        .expect("spawn python zip listing");
    assert!(output.status.success(), "the zip listing succeeded");
    let names: Vec<String> = serde_json::from_slice(&output.stdout).expect("a JSON name list");

    let defaults_root = root.join("python/hares/defaults");
    let shipped = collect_files(&defaults_root);
    assert!(!shipped.is_empty(), "the packaged tree carries files");
    let expected: Vec<String> = shipped
        .iter()
        .map(|path| {
            format!(
                "hares/defaults/{}",
                path.strip_prefix(&defaults_root)
                    .expect("under the tree")
                    .to_string_lossy()
            )
        })
        .collect();
    let top_level_defaults: Vec<&String> = names
        .iter()
        .filter(|name| name.split('/').next() == Some("defaults"))
        .collect();
    assert!(
        top_level_defaults.is_empty(),
        "no top-level defaults/ entry, got: {:?}",
        &top_level_defaults[..top_level_defaults.len().min(5)]
    );
    for file in &expected {
        assert!(
            names.iter().any(|name| name == file),
            "the wheel carries {file} under hares/defaults/"
        );
    }
}

fn collect_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read the tree") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_files(&path));
        } else {
            files.push(path);
        }
    }
    files.sort();
    files
}
