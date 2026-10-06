//! One file path in a test's own temporary directory, included by path into
//! each test target that needs it.

use std::path::PathBuf;

use tempfile::TempDir;

/// A path named `name` in a directory of its own, removed with everything in
/// it when the returned `TempDir` drops, panics included.
pub fn temp_file(name: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(name);
    (dir, path)
}
