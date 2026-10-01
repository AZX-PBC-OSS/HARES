//! Defaults-tree handling: provenance digest and the `[defaults_files]`
//! replacement directory.
//!
//! The engine does not report which defaults files it read, and it reads
//! some of them in OS `read_dir` order, which is not deterministic. The
//! provenance digest therefore covers every file in the defaults tree the
//! run actually used, in sorted relative-path order: a deterministic
//! superset of the store's read set, so a capture taken against a modified
//! or different defaults tree is always detectable.

use std::path::{Path, PathBuf};

use crate::error::FrameGoldenResult;
use crate::manifest::GoldenManifest;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Where the run's defaults live: the repository directory itself, or a
/// temporary tree of symlinks to it with the manifest's replacement files
/// copied in. Dropping the value removes the temporary tree.
pub struct DefaultsTree(DefaultsSource);

enum DefaultsSource {
    Repo(PathBuf),
    Temp(TempDir),
}

impl DefaultsTree {
    /// The directory the run reads defaults from.
    pub fn dir(&self) -> &Path {
        match &self.0 {
            DefaultsSource::Repo(dir) => dir,
            DefaultsSource::Temp(temp) => temp.path(),
        }
    }
}

/// Collects every file under `dir` as (relative path, bytes), recursively,
/// sorted by relative path.
fn collect_files(
    dir: &Path,
    relative: &str,
    out: &mut Vec<(String, Vec<u8>)>,
) -> FrameGoldenResult<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let rel = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        let meta = std::fs::metadata(&path)?;
        if meta.is_dir() {
            collect_files(&path, &rel, out)?;
        } else {
            let bytes = std::fs::read(&path)?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

/// One digest over every file in the tree: for each file in sorted
/// relative-path order, the relative path, a `0x00` separator, then the
/// file bytes.
pub fn digest_tree(dir: &Path) -> FrameGoldenResult<String> {
    let mut files = Vec::new();
    collect_files(dir, "", &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, bytes) in &files {
        hasher.update(rel.as_bytes());
        hasher.update([0x00]);
        hasher.update(bytes);
    }
    Ok(crate::digest::hex_digest(&hasher.finalize()))
}

fn symlink_or_copy(target: &Path, link: &Path) -> FrameGoldenResult<()> {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    #[cfg(not(unix))]
    std::fs::copy(target, link)?;
    Ok(())
}

/// Prepares the defaults directory the run reads: the repository directory
/// itself, or a temporary tree of symlinks to it with the manifest's
/// replacement files copied in.
pub fn prepare(root: &Path, manifest: &GoldenManifest) -> FrameGoldenResult<DefaultsTree> {
    let repo_defaults = crate::manifest::repo_path(root, &manifest.defaults);
    if manifest.defaults_files.is_empty() {
        return Ok(DefaultsTree(DefaultsSource::Repo(repo_defaults)));
    }

    let temp = TempDir::new()?;
    let mut files = Vec::new();
    collect_files(&repo_defaults, "", &mut files)?;
    for (rel, _) in &files {
        let link = temp.path().join(rel);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        symlink_or_copy(&repo_defaults.join(rel), &link)?;
    }
    for (defaults_rel, repo_file) in &manifest.defaults_files {
        let target = temp.path().join(defaults_rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The tree linked `target` into the repository's own defaults
        // tree, and fs::copy follows a destination symlink: the link must
        // go first, so the copy materializes a real file in the temporary
        // directory instead of overwriting the repository's file.
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            // A replacement may also name a file the repository tree does
            // not carry at all.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        std::fs::copy(crate::manifest::repo_path(root, repo_file), &target)?;
    }
    Ok(DefaultsTree(DefaultsSource::Temp(temp)))
}

/// The provenance digest for the tree a run used.
pub fn digest_used(tree: &DefaultsTree) -> FrameGoldenResult<String> {
    digest_tree(tree.dir())
}
