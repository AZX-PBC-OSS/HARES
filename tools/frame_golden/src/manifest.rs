//! Golden fixture manifests: `tests/fixtures/golden/<name>.toml`.
//!
//! The manifest names the exact feature set the fixture runs under, the
//! defaults directory, optional per-file defaults replacements, a
//! `SimulationConfig` table verbatim, an optional `[tariff]` table
//! (dwelling manifests only), and the homes; a fleet manifest also states
//! each home's weight and the resolution its products are bucketed at.
//! Unknown keys are rejected at every level the tool owns, so a typo
//! cannot silently change what a golden pins.

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

/// The resolution a fleet's products are bucketed at, as the manifest
/// spells it. Any other string is rejected at parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestResolution {
    FifteenMin,
    Hourly,
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
    /// Warm-up setting: `0` runs no warm-up and a positive value runs the
    /// engine's converging warm-up: the rule the Python binding applies
    /// (`crates/hares-python/src/py_dwelling.rs`: zero maps to
    /// `initialization_duration: None`, a positive value to `Some`). The
    /// engine ignores the value's magnitude: any positive setting runs the
    /// same fixed convergence loop, so the number of warm-up days is not
    /// chosen here.
    pub initialization_duration_s: u64,
    /// The home's sample weight: what the home contributes to the fleet
    /// aggregate. Required for kind "fleet" and rejected for kind
    /// "dwelling"; a NaN, infinite or negative value fails the run when
    /// `Fleet::with_sample_weights` classifies it.
    pub weight: Option<f64>,
    pub overrides: serde_json::Value,
}

/// The manifest's optional `[tariff]` table: the tariff a dwelling run is
/// billed under and the IANA zone its billing periods resolve in.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TariffConfig {
    /// Repository-relative JSON file deserialized into
    /// `hares_tariff::ElectricTariff` and checked with `validate()`.
    pub file: String,
    /// IANA zone name (for example `America/Denver`).
    pub zone: String,
}

impl TariffConfig {
    /// The zone as a `chrono_tz::Tz`, with the zone name in the error.
    fn zone_tz(&self) -> Result<chrono_tz::Tz, String> {
        self.zone
            .parse::<chrono_tz::Tz>()
            .map_err(|err| format!("zone {:?}: {err}", self.zone))
    }
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
    /// The resolution the fleet's products are bucketed at. Required for
    /// kind "fleet" and rejected for kind "dwelling"; the validator states
    /// both rules.
    #[serde(default)]
    pub resolution: Option<ManifestResolution>,
    /// The optional `[tariff]` table. `parse` checks its shape (dwelling
    /// manifests only, `file` and `zone` present, no other key) and parses
    /// `zone`; [`GoldenManifest::tariff`] reads and validates the file at
    /// run time.
    #[serde(default)]
    pub tariff: Option<TariffConfig>,
    /// Where the manifest was parsed from, for error messages; not part
    /// of the manifest text itself.
    #[serde(skip)]
    pub source: PathBuf,
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
        let mut manifest: Self =
            toml::from_str(text).map_err(|err| FrameGoldenError::Manifest {
                path: path.to_path_buf(),
                detail: err.to_string(),
            })?;
        manifest.source = path.to_path_buf();
        manifest.validate(path)?;
        Ok(manifest)
    }

    /// Loads and validates a manifest from disk. A manifest with a
    /// `[tariff]` table also has its tariff file read, parsed and
    /// validated here, so a bad tariff fails when the manifest loads.
    pub fn load(path: &Path) -> FrameGoldenResult<Self> {
        let text = std::fs::read_to_string(path)?;
        let manifest = Self::parse(path, &text)?;
        manifest.tariff(&repo_root(path))?;
        Ok(manifest)
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
        if let Some(tariff) = &self.tariff {
            if self.kind != ManifestKind::Dwelling {
                return Err(FrameGoldenError::Manifest {
                    path: path.to_path_buf(),
                    detail: format!(
                        "[tariff] is accepted for kind \"dwelling\" only, found kind {:?}",
                        self.kind
                    ),
                });
            }
            tariff
                .zone_tz()
                .map_err(|detail| FrameGoldenError::Manifest {
                    path: path.to_path_buf(),
                    detail: format!("[tariff] {detail}"),
                })?;
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
        }?;
        for home in &self.home {
            match (self.kind, home.weight) {
                (ManifestKind::Fleet, None) => {
                    return Err(FrameGoldenError::Manifest {
                        path: path.to_path_buf(),
                        detail: format!(
                            "[[home]] bldg_id {}: weight is required for kind \"fleet\"",
                            home.bldg_id
                        ),
                    });
                }
                (ManifestKind::Dwelling, Some(_)) => {
                    return Err(FrameGoldenError::Manifest {
                        path: path.to_path_buf(),
                        detail: format!(
                            "[[home]] bldg_id {}: weight is accepted for kind \"fleet\" only, found kind {:?}",
                            home.bldg_id, self.kind
                        ),
                    });
                }
                (ManifestKind::Fleet, Some(_)) | (ManifestKind::Dwelling, None) => {}
            }
        }
        match (self.kind, self.resolution) {
            (ManifestKind::Fleet, None) => Err(FrameGoldenError::Manifest {
                path: path.to_path_buf(),
                detail: "resolution is required for kind \"fleet\"".to_string(),
            }),
            (ManifestKind::Dwelling, Some(_)) => Err(FrameGoldenError::Manifest {
                path: path.to_path_buf(),
                detail: format!(
                    "resolution is accepted for kind \"fleet\" only, found kind {:?}",
                    self.kind
                ),
            }),
            (ManifestKind::Fleet, Some(_)) | (ManifestKind::Dwelling, None) => Ok(()),
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

    /// Reads, parses and validates the manifest's tariff file against the
    /// repository root: `file` is resolved with `repo_path`, deserialized
    /// strictly into `hares_tariff::ElectricTariff` (unknown keys included,
    /// so a misspelled key fails here) and checked with `validate()`.
    /// Returns the tariff with its zone, or `None` when the manifest
    /// carries no `[tariff]` table. Every failure names the manifest, the
    /// `[tariff]` field and the cause.
    pub fn tariff(
        &self,
        root: &Path,
    ) -> FrameGoldenResult<Option<(hares_tariff::ElectricTariff, chrono_tz::Tz)>> {
        let Some(config) = &self.tariff else {
            return Ok(None);
        };
        let manifest_path = self.source.clone();
        let fail = |detail: String| FrameGoldenError::Manifest {
            path: manifest_path.clone(),
            detail,
        };

        let file_path = repo_path(root, &config.file);
        let text = std::fs::read_to_string(&file_path).map_err(|err| {
            fail(format!(
                "[tariff] file {:?}: cannot be read: {err}",
                config.file
            ))
        })?;
        let tariff: hares_tariff::ElectricTariff = serde_json::from_str(&text)
            .map_err(|err| fail(format!("[tariff] file {:?}: {err}", config.file)))?;
        tariff.validate().map_err(|err| {
            fail(format!(
                "[tariff] file {:?}: validate() failed: {err}",
                config.file
            ))
        })?;
        let tz = config
            .zone_tz()
            .map_err(|detail| fail(format!("[tariff] {detail}")))?;
        Ok(Some((tariff, tz)))
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
