//! Capture and materialize orchestration.
//!
//! Both run a manifest through the adapter, build the golden document from
//! the run's products, write the committed `.golden.json` next to the
//! manifest, and (when asked) write the full frames as ZSTD Parquet under
//! the local frames directory. `materialize` additionally refuses to
//! produce frames whose digests differ from the committed golden: it
//! exists to obtain the exact reference frames of a capture.

use std::path::{Path, PathBuf};

use crate::adapter::{RunOutput, RunProducts, RunRequest};
use crate::digest::digest_frame;
use crate::error::{FrameGoldenError, FrameGoldenResult};
use crate::frames;
use crate::golden::{GoldenDoc, git_info};
use crate::manifest::{GoldenManifest, running_features};

/// Everything one capture produced: the document and where it was written.
pub struct Captured {
    pub doc: GoldenDoc,
    pub golden_path: PathBuf,
    pub frames_dir: Option<PathBuf>,
}

/// Builds the golden document for one run's products.
fn build_doc(
    manifest: &GoldenManifest,
    products: &RunProducts,
    repo_root: &Path,
) -> FrameGoldenResult<GoldenDoc> {
    let mut products_digests = std::collections::BTreeMap::new();
    for (name, frame) in &products.frames {
        products_digests.insert(name.clone(), digest_frame(&frame.batches)?);
    }
    let (git_head, git_dirty) = git_info(repo_root);
    Ok(GoldenDoc {
        kind: match products.kind {
            crate::manifest::ManifestKind::Dwelling => "dwelling".to_string(),
            crate::manifest::ManifestKind::Fleet => "fleet".to_string(),
        },
        features: manifest.feature_names(),
        defaults_dir: products.defaults_dir.clone(),
        defaults_digest: products.defaults_digest.clone(),
        git_head,
        git_dirty,
        metrics: products.metrics_rows.clone(),
        products: products_digests,
        health: products.health.clone(),
    })
}

/// Writes the full frame products as Parquet under `frames_dir`.
fn write_frames(products: &RunProducts, frames_dir: &Path) -> FrameGoldenResult<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (name, frame) in &products.frames {
        let path = frames_dir.join(format!("{name}.parquet"));
        frames::write_frame(&path, &frame.batches)?;
        written.push(path);
    }
    Ok(written)
}

/// Enforces the manifest's exact feature-set rule before any run: a
/// capture, materialize, compare or delta taken under different features
/// than the fixture declares would compare unrelated behaviour.
pub fn check_features(manifest: &GoldenManifest, manifest_path: &Path) -> FrameGoldenResult<()> {
    if !manifest.matches_running_features() {
        return Err(FrameGoldenError::FeatureSetMismatch {
            manifest: manifest_path.to_path_buf(),
            required: manifest.feature_names(),
            running: running_features(),
        });
    }
    Ok(())
}

fn load_and_check(manifest_path: &Path) -> FrameGoldenResult<GoldenManifest> {
    let manifest = GoldenManifest::load(manifest_path)?;
    check_features(&manifest, manifest_path)?;
    Ok(manifest)
}

/// Captures a manifest: runs it, writes the committed golden document next
/// to the manifest, and (when `frames_dir` is set) the full products under
/// it.
pub fn capture(
    manifest_path: &Path,
    repo_root: &Path,
    frames_dir: Option<&Path>,
) -> FrameGoldenResult<Captured> {
    let manifest = load_and_check(manifest_path)?;
    let products = adapter_run(&manifest, repo_root, RunOutput::Full, None)?;
    let doc = build_doc(&manifest, &products, repo_root)?;
    let golden_path = doc.write_next_to(manifest_path)?;
    let frames_dir = frames_dir.map(Path::to_path_buf);
    if let Some(dir) = &frames_dir {
        write_frames(&products, dir)?;
    }
    Ok(Captured {
        doc,
        golden_path,
        frames_dir,
    })
}

/// Materializes a manifest: re-runs it, requires the fresh digests to equal
/// the committed golden, and writes the full products for `delta` to
/// compare against later.
pub fn materialize(
    manifest_path: &Path,
    repo_root: &Path,
    frames_dir: &Path,
) -> FrameGoldenResult<Captured> {
    let manifest = load_and_check(manifest_path)?;
    let golden_path = committed_golden_path(manifest_path);
    let committed_bytes = std::fs::read(&golden_path).map_err(|err| {
        FrameGoldenError::MaterializedFrames(format!(
            "committed golden {} is unreadable ({}): run `frame_golden capture` first",
            golden_path.display(),
            err
        ))
    })?;
    let committed = GoldenDoc::from_bytes(&committed_bytes)?;

    let products = adapter_run(&manifest, repo_root, RunOutput::Full, None)?;
    let doc = build_doc(&manifest, &products, repo_root)?;
    if doc.products != committed.products
        || doc.metrics != committed.metrics
        || doc.health != committed.health
    {
        return Err(FrameGoldenError::MaterializedFrames(format!(
            "fresh digests differ from the committed golden {}: \
             re-capture instead of materializing a changed run",
            golden_path.display()
        )));
    }
    write_frames(&products, frames_dir)?;
    Ok(Captured {
        doc,
        golden_path,
        frames_dir: Some(frames_dir.to_path_buf()),
    })
}

/// The local full-frames directory for one fixture.
pub fn frames_dir_for(repo_root: &Path, name: &str) -> PathBuf {
    repo_root.join("target").join("golden").join(name)
}

/// The committed golden path for one manifest.
pub fn committed_golden_path(manifest_path: &Path) -> PathBuf {
    let stem = manifest_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{stem}.golden.json"))
}

fn adapter_run(
    manifest: &GoldenManifest,
    repo_root: &Path,
    output: RunOutput,
    duration_override_s: Option<i64>,
) -> FrameGoldenResult<RunProducts> {
    crate::adapter::run(RunRequest {
        manifest,
        repo_root,
        output,
        duration_override_s,
    })
}
