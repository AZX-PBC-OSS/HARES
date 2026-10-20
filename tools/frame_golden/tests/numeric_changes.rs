//! The docs' numeric-changes gate: every golden the "Numeric changes"
//! section of `docs/python.md` names exists as a fixture.
//!
//! The entry's `numeric_changes_name_existing_goldens`: the numeric
//! ledger's line names the goldens it re-captured; a rename of a fixture
//! or a stale line fails here instead of quietly pinning nothing.

use std::path::{Path, PathBuf};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The backtick-quoted golden names the docs' "Numeric changes" section
/// mentions, as the fixture names they stand for (`<name>` →
/// `tests/fixtures/golden/<name>.toml`). Excludes the non-golden quoted
/// terms (field names, conversions, paths).
fn named_goldens(docs: &str, section: &str) -> Vec<String> {
    let start = docs
        .find(section)
        .unwrap_or_else(|| panic!("the docs carry a {section} section"));
    let body = &docs[start..];
    let mut names = Vec::new();
    for part in body.split("`") {
        let part = part.trim();
        // A golden's name: a lowercase snake_case token that is not a path
        // or a unit and is named as a fixture in the section's own prose
        // ("captured", "identical", "re-captured").
        let is_name = !part.is_empty()
            && !part.contains('/')
            && !part.contains('(')
            && !part.contains(' ')
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
            && part.ends_with("_900s")
            || part.ends_with("_3600s")
            || part.ends_with("_hourly")
            || part.ends_with("_300s")
            || part.ends_with("_dst")
            || part == "resstock_2025_fleet"
            || part == "der_pv_battery_900s"
            || part == "der_pv_battery_ev_900s";
        if is_name && !names.iter().any(|n: &String| n == part) {
            names.push(part.to_string());
        }
    }
    names
}

/// Every golden the docs' "Numeric changes" section names exists as a
/// golden fixture.
#[test]
fn numeric_changes_name_existing_goldens() {
    let docs = std::fs::read_to_string(project_root().join("docs/python.md"))
        .expect("docs/python.md reads");
    let names = named_goldens(&docs, "## Numeric changes");
    assert!(
        !names.is_empty(),
        "the Numeric changes section names the goldens it re-captured"
    );
    let mut failures = Vec::new();
    for name in &names {
        let path = golden_toml(name);
        if !path.exists() {
            failures.push(format!("{name}: no fixture at {}", path.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "the Numeric changes section names goldens that do not exist:\n{}",
        failures.join("\n")
    );
}

fn golden_toml(name: &str) -> PathBuf {
    let root = project_root();
    let dir: &Path = Path::new("tests/fixtures/golden");
    root.join(dir).join(format!("{name}.toml"))
}
