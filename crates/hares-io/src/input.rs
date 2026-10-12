//! The input records a run's provenance carries: what inputs the parse
//! read, and the SHA-256 of the bytes read. A digest is over the content,
//! never over the path: the same bytes at a different path record the same
//! digest, and a path source keeps the path it was read from beside its
//! digest while content keeps the kind it was supplied as.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::weather::WeatherFormat;

/// Where an input came from. A path source keeps the path the input was
/// read from; content keeps the kind it was supplied as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRecordSource {
    /// A file the input was read from, kept as metadata beside the digest.
    Path(PathBuf),
    /// Content supplied as text.
    Text,
    /// Content supplied as bytes.
    Bytes,
}

/// One input's record: where it came from and the SHA-256 of the bytes
/// read, computed once when the input is parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRecord {
    pub source: InputRecordSource,
    pub sha256: [u8; 32],
}

impl InputRecord {
    /// Read the file at `path` and record the bytes' digest, the path kept
    /// as metadata.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] when the file cannot be read.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let bytes = fs::read(path)?;
        Ok(Self {
            source: InputRecordSource::Path(path.to_path_buf()),
            sha256: Sha256::digest(&bytes).into(),
        })
    }

    /// The digest as the 64 char lowercase hex string.
    #[must_use]
    pub fn hex(&self) -> String {
        hex(&self.sha256)
    }
}

/// Every input a run read, recorded when it was parsed. A schedule
/// generated from the HPXML has no record of its own: it is a function of
/// the HPXML record. The defaults digest runs over every defaults file the
/// load read, in load order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRecords {
    pub hpxml: InputRecord,
    /// `None` when the schedule is generated from the HPXML.
    pub schedule: Option<InputRecord>,
    pub weather: InputRecord,
    /// The format the weather was parsed as: the detected one for a path
    /// source, the given one for content, so replayed content keeps its
    /// format.
    pub weather_format: WeatherFormat,
    /// The digest over every defaults file the load read, in load order:
    /// each file contributes its relative path, a 0x00 separator and its
    /// bytes.
    pub defaults_files_sha256: [u8; 32],
    /// One record per defaults file a caller replaced before the load.
    pub defaults_replacements: Vec<(String, InputRecord)>,
}

/// The digest over a defaults load's files, in load order: relative path,
/// a 0x00 separator, bytes, concatenated across the files.
#[must_use]
pub fn defaults_files_digest(log: &[(String, Vec<u8>)]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (relative_path, bytes) in log {
        hasher.update(relative_path.as_bytes());
        hasher.update([0x00]);
        hasher.update(bytes);
    }
    hasher.finalize().into()
}

/// The bytes as the 64 char lowercase hex string.
#[must_use]
pub fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_record_hex_is_lowercase_sha256_hex() {
        let record = InputRecord {
            source: InputRecordSource::Bytes,
            sha256: Sha256::digest(b"abc").into(),
        };
        assert_eq!(
            record.hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn defaults_files_digest_follows_the_load_order_rule() {
        let empty = defaults_files_digest(&[]);
        assert_eq!(
            hex(&empty),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "no files read digests the empty input"
        );

        let mut hasher = Sha256::new();
        hasher.update(b"battery/a.toml");
        hasher.update([0x00]);
        hasher.update(b"alpha = 1\n");
        hasher.update(b"battery/b.toml");
        hasher.update([0x00]);
        hasher.update(b"beta = 2\n");
        let expected: [u8; 32] = hasher.finalize().into();

        let log = vec![
            ("battery/a.toml".to_string(), b"alpha = 1\n".to_vec()),
            ("battery/b.toml".to_string(), b"beta = 2\n".to_vec()),
        ];
        assert_eq!(
            defaults_files_digest(&log),
            expected,
            "each file contributes relative path, 0x00, bytes, in order"
        );

        let reversed = log.iter().rev().cloned().collect::<Vec<_>>();
        assert_ne!(
            defaults_files_digest(&reversed),
            expected,
            "the load order is part of the digest"
        );
    }

    #[test]
    fn input_record_from_path_hashes_the_file_bytes() {
        let dir = tempfile::tempdir().expect("temp directory");
        let path = dir.path().join("input.txt");
        std::fs::write(&path, b"content bytes").expect("write");
        let record = InputRecord::from_path(&path).expect("readable");
        assert_eq!(record.source, InputRecordSource::Path(path.clone()));
        assert_eq!(
            record.sha256[..],
            Sha256::digest(b"content bytes")[..],
            "the digest is over the file's bytes"
        );
    }
}
