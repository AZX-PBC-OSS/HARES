//! Golden fixture manifests: `tests/fixtures/golden/<name>.toml`.
//!
//! The manifest names the exact feature set the fixture runs under, the
//! defaults directory, optional per-file defaults replacements, a
//! `SimulationConfig` table verbatim, and the homes. Unknown keys are
//! rejected at every level the tool owns, so a typo cannot silently change
//! what a golden pins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{FrameGoldenError, FrameGoldenResult};

/// The simulation features a fixture runs under. Only `dst` changes engine
/// behaviour today; a new behaviour-changing feature extends this enum,
/// which rejects any other spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Feature {
    Dst,
}

impl Feature {
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::Dst => "dst",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestKind {
    Dwelling,
    Fleet,
}

/// One `[[home]]` entry. Paths are repository-relative; `overrides` is the
/// engine's equipment override map verbatim.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeEntry {
    pub bldg_id: i64,
    pub hpxml: String,
    pub schedule: String,
    pub weather: String,
    pub initialization_duration_s: u64,
    pub overrides: serde_json::Value,
}

/// A golden fixture manifest, parsed with unknown keys rejected.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenManifest {
    pub kind: ManifestKind,
    pub features: Vec<Feature>,
    pub defaults: String,
    #[serde(default)]
    pub defaults_files: BTreeMap<String, String>,
    pub simulation: toml::Table,
    /// Defaulted so a manifest with no `[[home]]` reaches the domain
    /// validator (which states the per-kind rule) instead of a generic
    /// serde missing-field error.
    #[serde(default)]
    pub home: Vec<HomeEntry>,
}

/// Every field of `hares_io::SimulationConfig`. A manifest key outside this
/// list is rejected instead of being silently dropped by serde defaults.
const SIMULATION_KEYS: &[&str] = &[
    "start_time",
    "duration",
    "time_res",
    "output_verbosity",
    "output_path",
    "write_output",
    "output_format",
    "output_chunk_size",
    "setpoint_deadband_c",
    "master_seed",
    "retain_batches",
    "civil_timezone",
    "rotation",
    "site_location",
];

/// The feature set the running binary was built with. The tool's features
/// forward to `hares-core`, so this is also the engine's feature set.
pub fn running_features() -> Vec<String> {
    if cfg!(feature = "dst") {
        vec!["dst".to_string()]
    } else {
        Vec::new()
    }
}

impl GoldenManifest {
    /// Parses a manifest from bytes with unknown keys rejected everywhere.
    pub fn parse(path: &Path, text: &str) -> FrameGoldenResult<Self> {
        let manifest: Self = toml::from_str(text).map_err(|err| FrameGoldenError::Manifest {
            path: path.to_path_buf(),
            detail: err.to_string(),
        })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    /// Loads and validates a manifest from disk.
    pub fn load(path: &Path) -> FrameGoldenResult<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(path, &text)
    }

    fn validate(&self, path: &Path) -> FrameGoldenResult<()> {
        for key in self.simulation.keys() {
            if !SIMULATION_KEYS.contains(&key.as_str()) {
                return Err(FrameGoldenError::Manifest {
                    path: path.to_path_buf(),
                    detail: format!(
                        "unknown key {key:?} in [simulation]: known keys are {SIMULATION_KEYS:?}"
                    ),
                });
            }
        }
        let count = self.home.len();
        match self.kind {
            ManifestKind::Dwelling if count != 1 => Err(FrameGoldenError::Manifest {
                path: path.to_path_buf(),
                detail: format!("kind \"dwelling\" requires exactly one [[home]], found {count}"),
            }),
            ManifestKind::Fleet if count < 2 => Err(FrameGoldenError::Manifest {
                path: path.to_path_buf(),
                detail: format!(
                    "kind \"fleet\" requires two or more [[home]] entries, found {count}"
                ),
            }),
            ManifestKind::Dwelling | ManifestKind::Fleet => Ok(()),
        }
    }

    /// The manifest's feature set as strings, in manifest order.
    pub fn feature_names(&self) -> Vec<String> {
        self.features
            .iter()
            .map(|feature| feature.as_str().to_string())
            .collect()
    }

    /// True when the manifest's feature set equals the running binary's.
    pub fn matches_running_features(&self) -> bool {
        self.feature_names() == running_features()
    }

    /// Parses the `[simulation]` table through `SimulationConfig::from_toml`
    /// so its validation rules (positive, time_res-divisible duration and
    /// the verbosity cap) apply exactly as they do anywhere else.
    pub fn simulation_config(&self) -> FrameGoldenResult<hares_io::SimulationConfig> {
        let text = toml::to_string(&self.simulation)
            .map_err(|err| FrameGoldenError::SimulationConfig(err.to_string()))?;
        hares_io::SimulationConfig::from_toml(&text)
            .map_err(|err| FrameGoldenError::SimulationConfig(err.to_string()))
    }
}

/// Finds the repository root: the nearest ancestor of the manifest that
/// holds a `.git` entry, falling back to the workspace root baked in at
/// compile time (the crate sits at `tools/frame_golden`, two levels below
/// the root) so a manifest outside the repository (the smoke manifest)
/// still resolves repository-relative paths.
pub fn repo_root(manifest_path: &Path) -> PathBuf {
    let mut dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    loop {
        if dir.join(".git").exists() {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    compile_time_repo_root()
}

fn compile_time_repo_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(|tools| tools.parent())
        .map(Path::to_path_buf)
        .expect("crate dir sits two levels below the repository root")
}

/// Resolves a repository-relative path against the repository root.
pub fn repo_path(root: &Path, relative: &str) -> PathBuf {
    root.join(relative)
}

/// Resolves a `<name>` (or explicit manifest path) to a manifest file: an
/// existing file is used as is, anything else names a fixture under
/// `tests/fixtures/golden/`.
pub fn resolve_manifest(root: &Path, name: &str) -> PathBuf {
    let as_path = PathBuf::from(name);
    if as_path.is_file() {
        return as_path;
    }
    root.join("tests")
        .join("fixtures")
        .join("golden")
        .join(format!("{name}.toml"))
}

/// Lists the manifest files in a directory of fixtures.
pub fn list_manifests(dir: &Path) -> FrameGoldenResult<Vec<PathBuf>> {
    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            manifests.push(path);
        }
    }
    manifests.sort();
    Ok(manifests)
}

/// Selects the manifests whose feature set equals `features` exactly.
pub fn select_manifests(dir: &Path, features: &[String]) -> FrameGoldenResult<Vec<PathBuf>> {
    let mut selected = Vec::new();
    for path in list_manifests(dir)? {
        let manifest = GoldenManifest::load(&path)?;
        if manifest.feature_names() == *features {
            selected.push(path);
        }
    }
    Ok(selected)
}
