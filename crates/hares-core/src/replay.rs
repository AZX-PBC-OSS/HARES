//! The replay export: a run's resolved configuration, inputs, seeds and
//! build identity, written into a directory a fresh construction reads to
//! reproduce the run exactly (`03-output-reporting.md` OR-16).
//!
//! The export carries what the run's provenance records and nothing else:
//! the validated `SimulationConfig` the run ran under (the master seed in
//! effect; every random stream the run drew derives from it and the
//! building id), the inputs' digests with their content, the attached
//! tariff and the build identity. `load` rebuilds the
//! `DwellingConfig` from a directory alone and
//! verifies the defaults tree the replay will read against the digest the
//! run recorded, so a replay either reproduces the run or fails naming
//! what it cannot reproduce.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hares_io::{
    DefaultsSource, InputRecord, InputSource, SimulationConfig, WeatherFormat, WeatherInput,
};
use hares_types::HaresError;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::dwelling::{Dwelling, DwellingConfig, DwellingSettings};
use crate::result::{BuildInfo, RunResult, TariffRecord};

/// The manifest's schema version: bumped when the manifest's shape
/// changes in a way an older export cannot load. Version 2 dropped the
/// `initialization_duration_s` key: the warm-up lives in the embedded
/// `SimulationConfig`'s `warmup` field, and an older export's key is a
/// removed key.
const REPLAY_SCHEMA_VERSION: u32 = 2;

/// The stored file names the export writes the inputs under, inside the
/// replay directory.
const HPXML_FILE: &str = "inputs/hpxml.xml";
const SCHEDULE_FILE: &str = "inputs/schedule.csv";
const WEATHER_BASE: &str = "inputs/weather";
const DEFAULTS_DIR: &str = "defaults";

/// The build identity as read back from `replay.toml`: `BuildInfo` with
/// owned strings (`BuildInfo`'s `&'static str` fields cannot be
/// deserialised).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedBuild {
    /// The crate version the build compiled.
    pub version: String,
    /// The git commit the build compiled from; `None` when built without
    /// a git work tree.
    pub commit: Option<String>,
    /// Whether the source tree the build compiled had tracked edits;
    /// `None` when git or `.git` is absent.
    pub dirty: Option<bool>,
    /// The build profile (`debug` or `release`).
    pub profile: String,
    /// The features the build enabled, sorted.
    pub features: Vec<String>,
    /// The compiler's version line (`rustc -V`).
    pub rustc: String,
}

impl RecordedBuild {
    fn from_build_info(build: &BuildInfo) -> Self {
        Self {
            version: build.version.to_string(),
            commit: build.commit.map(str::to_string),
            dirty: build.dirty,
            profile: build.profile.to_string(),
            features: build.features.iter().map(|f| (*f).to_string()).collect(),
            rustc: build.rustc.to_string(),
        }
    }
}

/// One exported input: its file name in the replay directory and the
/// digest the run's parse recorded over the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredInput {
    file: String,
    sha256: String,
}

/// One exported defaults replacement: the tree-relative path it replaces,
/// the file name it is stored under in the replay directory, and the
/// digest the parse recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredReplacement {
    relative: String,
    file: String,
    sha256: String,
}

/// The exported inputs: every input's stored file and digest, the format
/// the weather was parsed as, and the defaults load's composed digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredInputs {
    hpxml: StoredInput,
    schedule: Option<StoredInput>,
    weather: StoredInput,
    weather_format: WeatherFormat,
    defaults_files_sha256: String,
    defaults_replacements: Vec<StoredReplacement>,
}

/// The private manifest `replay.toml` holds: the settings the run ran
/// under (the validated config, whose `warmup` field carries the warm-up,
/// the building id and the patches; the overrides travel in
/// `overrides.json` because a JSON `null` has no TOML form), the exported
/// inputs, the attached tariff's zone and the step it attached at, and the
/// build identity.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayManifest {
    schema_version: u32,
    bldg_id: i64,
    patches: Option<hares_io::HpxmlDataPatches>,
    simulation: SimulationConfig,
    inputs: StoredInputs,
    tariff_zone: Option<String>,
    tariff_attach_step: Option<u64>,
    build: RecordedBuild,
}

/// A run's replay: everything a fresh construction needs, read back from
/// an export directory.
#[derive(Debug)]
pub struct Replay {
    /// The configuration to build: the inputs point into the export
    /// directory (and the defaults tree at the `defaults_base` the
    /// loader was given), the settings are the run's own.
    pub config: DwellingConfig,
    /// The tariff the run attached, attached at its recorded step: the
    /// steps before it replay tariff-less, the way the run ran them.
    pub tariff: Option<TariffRecord>,
    /// The build that produced the exported run. The replayed run's own
    /// provenance records the engine that replayed it.
    pub recorded_build: RecordedBuild,
}

impl Replay {
    /// Builds the replayed dwelling: the config's construction path,
    /// then the tariff attached at its recorded step. An attach recorded
    /// after step 0 replays the steps before it (tariff-less, the way
    /// the run ran them) so the attach happens with the dwelling's clock
    /// exactly where the run's was: the attach's own semantics (the
    /// prorated period it joins) fold from the recorded step, and the
    /// billing reproduces on bits.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the construction, a replayed step or the
    /// tariff attach fails.
    pub fn build(&self) -> Result<Dwelling, HaresError> {
        let mut dwelling = Dwelling::from_config(self.config.clone())?;
        if let Some(record) = &self.tariff {
            let tz: chrono_tz::Tz = record.zone.parse().map_err(|_| {
                HaresError::Tariff(format!(
                    "the exported tariff's zone '{}' is not an IANA name chrono-tz knows",
                    record.zone
                ))
            })?;
            for _ in 0..record.attach_step {
                dwelling.step()?;
            }
            dwelling.set_tariff(record.tariff.clone(), tz)?;
        }
        Ok(dwelling)
    }
}

/// The weather file's stored name: the format's own extension (the load
/// passes the recorded format explicitly, so the name is the human-facing
/// record, nothing reads it for detection).
fn weather_file_name(format: WeatherFormat) -> String {
    match format {
        WeatherFormat::Epw => format!("{WEATHER_BASE}.epw"),
        WeatherFormat::Psm3 | WeatherFormat::ResStockCsv | WeatherFormat::Tmy3 => {
            format!("{WEATHER_BASE}.csv")
        }
    }
}

/// Reads an input's bytes for the export. A path source reads the file at
/// the recorded path and its digest must equal the one recorded at parse
/// (a moved, edited or deleted file is not the input the run read); a
/// content source's bytes are the record's own.
fn export_bytes(record: &InputRecord, what: &str) -> Result<Vec<u8>, HaresError> {
    let bytes = match &record.source {
        hares_io::InputRecordSource::Path(path) => std::fs::read(path).map_err(|err| {
            HaresError::Io(format!(
                "the {what} input '{}' could not be read for the replay export: {err}",
                path.display()
            ))
        })?,
        hares_io::InputRecordSource::Text(text) => text.as_bytes().to_vec(),
        hares_io::InputRecordSource::Bytes(content) => content.to_vec(),
    };
    let digest: [u8; 32] = sha2::Sha256::digest(&bytes).into();
    if digest != record.sha256 {
        let named = match &record.source {
            hares_io::InputRecordSource::Path(path) => {
                format!("the {what} input at '{}'", path.display())
            }
            hares_io::InputRecordSource::Text(_) | hares_io::InputRecordSource::Bytes(_) => {
                format!("the {what} input's content")
            }
        };
        return Err(HaresError::Io(format!(
            "{named} changed since the run parsed it: the parse recorded digest {}, \
             the input's content now digests {}",
            hares_io::input::hex(&record.sha256),
            hares_io::input::hex(&digest),
        )));
    }
    Ok(bytes)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), HaresError> {
    std::fs::write(path, bytes)
        .map_err(|err| HaresError::Io(format!("'{}' could not be written: {err}", path.display())))
}

/// Stores one input's content under `relative` in the replay directory
/// and returns the manifest's record of it.
fn store_input(
    dir: &Path,
    relative: &str,
    record: &InputRecord,
    what: &str,
) -> Result<StoredInput, HaresError> {
    let bytes = export_bytes(record, what)?;
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            HaresError::Io(format!(
                "'{}' could not be created: {err}",
                parent.display()
            ))
        })?;
    }
    write_file(&path, &bytes)?;
    Ok(StoredInput {
        file: relative.to_string(),
        sha256: hares_io::input::hex(&record.sha256),
    })
}

/// Writes the run's replay export into `dir`
/// ([`RunResult::export_replay`](crate::RunResult::export_replay)).
pub(crate) fn export(result: &RunResult, dir: &Path) -> Result<(), HaresError> {
    if !result.provenance.replay_blockers.is_empty() {
        return Err(HaresError::NotReplayable {
            blockers: result.provenance.replay_blockers.clone(),
        });
    }
    let Some(inputs) = result.provenance.inputs.as_ref() else {
        return Err(HaresError::Io(
            "the run's provenance records no inputs (the synthetic TOML path reads the \
             TOML, which the input records do not carry): there is nothing to export \
             for a replay"
                .to_string(),
        ));
    };
    let settings = &result.provenance.settings;

    std::fs::create_dir_all(dir).map_err(|err| {
        HaresError::Io(format!("'{}' could not be created: {err}", dir.display()))
    })?;

    // The inputs' content, verbatim: the HPXML, the schedule when the run
    // read one, the weather, and each overridden defaults table. The
    // defaults tree itself is not copied.
    let hpxml_stored = store_input(dir, HPXML_FILE, &inputs.hpxml, "HPXML")?;
    let schedule_stored = match &inputs.schedule {
        Some(record) => Some(store_input(dir, SCHEDULE_FILE, record, "schedule")?),
        None => None,
    };
    let weather_file = weather_file_name(inputs.weather_format);
    let weather_stored = store_input(dir, &weather_file, &inputs.weather, "weather")?;
    let mut replacements = Vec::new();
    for (relative, record) in &inputs.defaults_replacements {
        let stored = format!("{DEFAULTS_DIR}/{relative}");
        let record_stored = store_input(dir, &stored, record, "defaults replacement")?;
        replacements.push(StoredReplacement {
            relative: relative.clone(),
            file: record_stored.file,
            sha256: record_stored.sha256,
        });
    }

    // The overrides, as JSON (a JSON null has no TOML form); absent when
    // the run had none.
    if let Some(overrides) = &settings.overrides {
        let json = serde_json::to_string_pretty(overrides)
            .map_err(|err| HaresError::Io(format!("the overrides' JSON: {err}")))?;
        write_file(&dir.join("overrides.json"), json.as_bytes())?;
    }

    // The attached tariff, as JSON; absent when the run attached none.
    if let Some(record) = &result.provenance.tariff {
        let json = serde_json::to_string_pretty(&record.tariff)
            .map_err(|err| HaresError::Io(format!("the tariff's JSON: {err}")))?;
        write_file(&dir.join("tariff.json"), json.as_bytes())?;
    }

    // The config's reports: a callback report replays as a Memory report
    // (the replayed run retains what the original delivered to the
    // callback), a file destination writes inside the replay directory.
    let mut config = settings.sim_config.clone();
    for report in &mut config.reports {
        match &report.destination {
            hares_io::Destination::Callback => {
                report.destination = hares_io::Destination::Memory;
            }
            hares_io::Destination::Parquet { overwrite, .. } => {
                report.destination = hares_io::Destination::Parquet {
                    path: PathBuf::from(format!("{}.parquet", report.name)),
                    overwrite: *overwrite,
                };
            }
            hares_io::Destination::Csv { overwrite, .. } => {
                report.destination = hares_io::Destination::Csv {
                    path: PathBuf::from(format!("{}.csv", report.name)),
                    overwrite: *overwrite,
                };
            }
            hares_io::Destination::Memory => {}
        }
    }

    let manifest = ReplayManifest {
        schema_version: REPLAY_SCHEMA_VERSION,
        bldg_id: settings.bldg_id,
        patches: settings.patches.clone(),
        simulation: config,
        inputs: StoredInputs {
            hpxml: hpxml_stored,
            schedule: schedule_stored,
            weather: weather_stored,
            weather_format: inputs.weather_format,
            defaults_files_sha256: hares_io::input::hex(&inputs.defaults_files_sha256),
            defaults_replacements: replacements,
        },
        tariff_zone: result.provenance.tariff.as_ref().map(|r| r.zone.clone()),
        tariff_attach_step: result.provenance.tariff.as_ref().map(|r| r.attach_step),
        build: RecordedBuild::from_build_info(&result.provenance.build),
    };
    let toml = toml::to_string_pretty(&manifest)
        .map_err(|err| HaresError::Io(format!("the replay manifest's TOML: {err}")))?;
    write_file(&dir.join("replay.toml"), toml.as_bytes())?;
    Ok(())
}

/// Reads a replay export (`hares_core::replay::load`).
///
/// # Errors
///
/// [`HaresError::Io`] naming the mismatch when the defaults tree at
/// `defaults_base` digests differently from the one the exported run
/// read, and when the directory or any file cannot be read.
pub fn load(dir: &Path, defaults_base: &Path) -> Result<Replay, HaresError> {
    let toml = std::fs::read_to_string(dir.join("replay.toml")).map_err(|err| {
        HaresError::Io(format!(
            "'{}' could not be read: {err}",
            dir.join("replay.toml").display()
        ))
    })?;
    let manifest: ReplayManifest = toml::from_str(&toml)
        .map_err(|err| HaresError::Io(format!("the replay manifest could not be read: {err}")))?;
    if manifest.schema_version != REPLAY_SCHEMA_VERSION {
        return Err(HaresError::Io(format!(
            "the replay manifest's schema version is {} and this engine reads {}",
            manifest.schema_version, REPLAY_SCHEMA_VERSION
        )));
    }

    // The exported inputs, read back from the export directory as content.
    // Every stored file's digest is compared against the manifest's record
    // first: a flipped byte must fail the load naming the path and both
    // digests, never replay on silently different inputs.
    let read_stored = |stored: &StoredInput, what: &str| -> Result<Arc<str>, HaresError> {
        let path = dir.join(&stored.file);
        let bytes = std::fs::read(&path).map_err(|err| {
            HaresError::Io(format!(
                "the replay's {what} input '{}' could not be read: {err}",
                path.display()
            ))
        })?;
        let digest: [u8; 32] = sha2::Sha256::digest(&bytes).into();
        let recorded = decode_hex_digest(&stored.sha256)?;
        if digest != recorded {
            return Err(HaresError::Io(format!(
                "the replay's {what} input '{}' is not the file the export stored: the manifest \
                 records digest {}, the file on disk digests {}",
                path.display(),
                stored.sha256,
                hares_io::input::hex(&digest),
            )));
        }
        String::from_utf8(bytes)
            .map(Into::into)
            .map_err(|err| HaresError::Io(format!("the replay's {what} input is not UTF-8: {err}")))
    };

    // The defaults: the tree at defaults_base, the export's replacement
    // files instead of the base's own. The load verifies the digest over
    // what the tree's load reads against the one the run recorded, so a
    // different or changed tree fails naming the mismatch instead of
    // standing in silently.
    let mut files: BTreeMap<String, InputSource> = BTreeMap::new();
    for replacement in &manifest.inputs.defaults_replacements {
        let path = dir.join(&replacement.file);
        let bytes = std::fs::read(&path).map_err(|err| {
            HaresError::Io(format!(
                "the replay's defaults replacement '{}' could not be read: {err}",
                path.display()
            ))
        })?;
        let digest: [u8; 32] = sha2::Sha256::digest(&bytes).into();
        let recorded = decode_hex_digest(&replacement.sha256)?;
        if digest != recorded {
            return Err(HaresError::Io(format!(
                "the replay's defaults replacement '{}' is not the file the export stored: the \
                 manifest records digest {}, the file on disk digests {}",
                path.display(),
                replacement.sha256,
                hares_io::input::hex(&digest),
            )));
        }
        let text = String::from_utf8(bytes).map_err(|err| {
            HaresError::Io(format!(
                "the replay's defaults replacement '{}' is not valid UTF-8: {err}",
                path.display()
            ))
        })?;
        files.insert(replacement.relative.clone(), InputSource::Text(text.into()));
    }
    let defaults = DefaultsSource {
        base: defaults_base.to_path_buf(),
        files,
    };
    let parsed_defaults = hares_io::ParsedDefaults::parse(&defaults)?;
    let actual = parsed_defaults.composed_digest()?;
    let recorded = decode_hex_digest(&manifest.inputs.defaults_files_sha256)?;
    if actual != recorded {
        return Err(HaresError::Io(format!(
            "the defaults tree at '{}' is not the one the run read: the run recorded \
             digest {}, the tree's load digests {}",
            defaults_base.display(),
            manifest.inputs.defaults_files_sha256,
            hares_io::input::hex(&actual),
        )));
    }

    let hpxml = InputSource::Text(read_stored(&manifest.inputs.hpxml, "HPXML")?);
    let schedule = match &manifest.inputs.schedule {
        Some(stored) => Some(InputSource::Text(read_stored(stored, "schedule")?)),
        None => None,
    };
    let weather = WeatherInput::Text {
        content: read_stored(&manifest.inputs.weather, "weather")?,
        format: manifest.inputs.weather_format,
    };

    // The config the run ran under: the exported simulation config, the
    // file destinations resolved inside the export directory, the
    // overrides when the export carries them.
    let overrides = match std::fs::read_to_string(dir.join("overrides.json")) {
        Ok(json) => Some(serde_json::from_str(&json).map_err(|err| {
            HaresError::Io(format!(
                "the replay's overrides.json could not be read: {err}"
            ))
        })?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(HaresError::Io(format!(
                "'{}' could not be read: {err}",
                dir.join("overrides.json").display()
            )));
        }
    };
    let mut config = manifest.simulation;
    for report in &mut config.reports {
        if let Some(path) = report.destination.path()
            && path.is_relative()
        {
            let relative = path.to_path_buf();
            report.destination = match &report.destination {
                hares_io::Destination::Parquet { overwrite, .. } => {
                    hares_io::Destination::Parquet {
                        path: dir.join(relative),
                        overwrite: *overwrite,
                    }
                }
                hares_io::Destination::Csv { overwrite, .. } => hares_io::Destination::Csv {
                    path: dir.join(relative),
                    overwrite: *overwrite,
                },
                other => other.clone(),
            };
        }
    }

    let tariff = match (
        &manifest.tariff_zone,
        std::fs::read_to_string(dir.join("tariff.json")),
    ) {
        (Some(zone), Ok(json)) => Some(TariffRecord {
            tariff: serde_json::from_str(&json).map_err(|err| {
                HaresError::Io(format!("the replay's tariff.json could not be read: {err}"))
            })?,
            zone: zone.clone(),
            attach_step: manifest.tariff_attach_step.unwrap_or(0),
        }),
        (Some(zone), Err(err)) => {
            return Err(HaresError::Io(format!(
                "the replay manifest names tariff zone '{zone}' but the export's \
                     tariff.json could not be read: {err}"
            )));
        }
        (None, _) => None,
    };

    let settings = DwellingSettings::new(config, overrides, manifest.bldg_id, manifest.patches);
    Ok(Replay {
        config: DwellingConfig {
            hpxml,
            schedule,
            weather,
            defaults,
            settings,
        },
        tariff,
        recorded_build: manifest.build,
    })
}

/// The 64 char hex digest the manifest carries, decoded.
fn decode_hex_digest(hex: &str) -> Result<[u8; 32], HaresError> {
    fn value(c: u8) -> Result<u8, HaresError> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            other => Err(HaresError::Io(format!(
                "the replay manifest's digest is not lowercase hex: {}",
                other as char
            ))),
        }
    }
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return Err(HaresError::Io(format!(
            "the replay manifest's digest is {} characters, expected 64",
            bytes.len()
        )));
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = (value(bytes[index * 2])? << 4) | value(bytes[index * 2 + 1])?;
    }
    Ok(digest)
}
