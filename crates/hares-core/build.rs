//! The build script recording the crate's compile-time build identity:
//! the commit the workspace's git tree is at, its dirty flag, the build
//! profile, the enabled features and the rustc version. The identity is
//! written as one expression into `OUT_DIR`, which `result.rs` includes
//! into `BuildInfo::get()`, so every result records the identity of the
//! build that produced it.
//!
//! The commit is read from the git tree's files (the parsing lives in
//! `build_support/git.rs`, included here), never from a git process; the
//! dirty flag is the porcelain output of one `git status` run, parsed by
//! the same pure function, `None` when git or `.git` is absent. The
//! rerun paths are relative to this package, resolved from
//! `CARGO_MANIFEST_DIR` to the workspace root, and emitted only when
//! they exist.

#[path = "build_support/git.rs"]
mod git;

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let workspace_root = manifest_dir.join("../..");

    for path in git::rerun_paths(&workspace_root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let git_dir = git::resolve_git_dir(&workspace_root);
    let commit = git_dir
        .as_ref()
        .and_then(|git_dir| git::resolve_head(git_dir));
    let dirty = git_dir.is_some().then(|| {
        Command::new("git")
            .arg("-C")
            .arg(&workspace_root)
            .args(["status", "--porcelain", "--untracked-files=no"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| git::porcelain_is_dirty(&String::from_utf8_lossy(&output.stdout)))
    });
    let dirty = dirty.flatten();

    let mut features: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(str::to_lowercase))
        .collect();
    features.sort();
    features.dedup();

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let rustc = Command::new(&rustc)
        .arg("-V")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set"));
    let profile = std::env::var("PROFILE").expect("PROFILE is set");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set");
    std::fs::write(
        out_dir.join("build_info.rs"),
        build_info_expression(&version, &commit, dirty, &profile, &features, &rustc),
    )
    .expect("the build identity writes into OUT_DIR");
}

/// The generated expression `BuildInfo::get()` includes: the literal
/// identity of this build, string-escaped by the debug formatting.
fn build_info_expression(
    version: &str,
    commit: &Option<String>,
    dirty: Option<bool>,
    profile: &str,
    features: &[String],
    rustc: &str,
) -> String {
    let commit = match commit {
        Some(commit) => format!("Some({commit:?})"),
        None => "None".to_string(),
    };
    let dirty = match dirty {
        Some(dirty) => format!("Some({dirty})"),
        None => "None".to_string(),
    };
    let features: Vec<String> = features
        .iter()
        .map(|feature| format!("{feature:?}"))
        .collect();
    format!(
        "BuildInfo {{\n    version: {version:?},\n    commit: {commit},\n    dirty: {dirty},\n    profile: {profile:?},\n    features: &[{}],\n    rustc: {rustc:?},\n}}\n",
        features.join(", ")
    )
}
