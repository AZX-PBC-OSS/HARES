//! The removed `resample_overrides` key is an error on every surface that
//! carried it. This file owns the synthetic TOML surface: a dwelling table
//! holding the key fails construction before deserialising, with the
//! removed-keys table's message naming the Python adapter that holds the
//! weather now. The Python surfaces' arms of the same test live in
//! tests/python/test_py_config.py; the scoped lookup's own tests live with
//! the table in hares-io.

use std::path::PathBuf;

use hares_core::Dwelling;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn removed_key_resample_overrides_is_an_error() {
    let source = project_root().join("tests/fixtures/bestest/600.toml");
    let toml_str = std::fs::read_to_string(&source).expect("read 600.toml");
    let held = format!("resample_overrides = {{ dry_bulb = \"zoh\" }}\n\n{toml_str}");
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("600_with_removed_key.toml");
    std::fs::write(&path, &held).expect("write the fixture copy");

    let err = match Dwelling::from_toml_config(&path) {
        Err(err) => err,
        Ok(_) => panic!("the removed key must stop construction"),
    };
    let message = err.to_string();
    assert!(
        message.contains("resample_overrides"),
        "the error names the key: {message}"
    );
    assert!(
        message.contains("hares.adapters.ochre.hold_weather"),
        "the error points at the Python adapter: {message}"
    );
}
