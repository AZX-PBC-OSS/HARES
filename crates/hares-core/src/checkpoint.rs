//! Simulation checkpoint schema and file I/O.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checksum;
use hares_types::{HaresError, ZoneId};
use serde::{Deserialize, Serialize};

/// Bump whenever checkpoint schema or state encoding changes.
pub const CHECKPOINT_VERSION: u32 = 4;

/// Serializable snapshot of all state required to resume a dwelling run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DwellingCheckpoint {
    pub format_version: u32,
    pub bldg_id: i64,
    pub timestep_index: u64,
    pub equipment_states: Vec<Vec<u8>>,
    pub rng_state: [u8; 32],
    pub envelope_state: Vec<f64>,
    /// Per-zone humidity ratios. Each entry is `(ZoneId, humidity_ratio)`.
    pub humidity_states: Vec<(ZoneId, f64)>,
    pub fluid_states: Vec<f64>,
    pub rng_stream: u64,
    pub rng_word_pos: u128,
    pub thermal_last_u: Vec<f64>,
    /// Per-exterior-surface converged LWR surface temperatures [°C].
    pub lwr_t_prev_c: Vec<f64>,
}

impl DwellingCheckpoint {
    /// Persist checkpoint atomically with SHA-256 integrity checksum.
    pub fn save(&self, path: &Path) -> Result<(), HaresError> {
        let json_bytes = serde_json::to_vec(self)
            .map_err(|err| HaresError::Io(format!("checkpoint serialize failed: {err}")))?;

        #[cfg(feature = "observe")]
        tracing::debug!(
            checkpoint_path = %path.display(),
            sha256 = %checksum::compute_sha256_hex(&json_bytes),
            "checkpoint saved with integrity checksum",
        );

        let file_bytes = checksum::write_with_sha256(&json_bytes);
        let tmp_path = temp_checkpoint_path(path);
        fs::write(&tmp_path, &file_bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint temp write failed: {err}")))?;
        fs::rename(&tmp_path, path)
            .map_err(|err| HaresError::Io(format!("checkpoint atomic rename failed: {err}")))?;
        Ok(())
    }

    /// Load checkpoint from disk, verifying SHA-256 integrity before
    /// deserialisation.
    pub fn load(path: &Path) -> Result<Self, HaresError> {
        let file_bytes = fs::read(path)
            .map_err(|err| HaresError::Io(format!("checkpoint read failed: {err}")))?;

        let json_bytes = checksum::verify_sha256(&file_bytes).map_err(|msg| {
            #[cfg(feature = "observe")]
            tracing::info!(
                checkpoint_path = %path.display(),
                sha256_pass = false,
                "checkpoint integrity check FAILED: {msg}",
            );
            HaresError::Io(format!("checkpoint file '{}' {msg}", path.display()))
        })?;

        #[cfg(feature = "observe")]
        tracing::info!(
            checkpoint_path = %path.display(),
            sha256_pass = true,
            "checkpoint integrity check passed",
        );

        let cp: DwellingCheckpoint = serde_json::from_slice(json_bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint parse failed: {err}")))?;
        if cp.format_version != CHECKPOINT_VERSION {
            return Err(HaresError::Io(format!(
                "checkpoint version mismatch: file={}, expected={}",
                cp.format_version, CHECKPOINT_VERSION
            )));
        }
        Ok(cp)
    }
}

fn temp_checkpoint_path(path: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tid = std::thread::current().id();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dwelling.chkpt");
    let tmp_name = format!("{file_name}.{nonce}.{tid:?}.tmp");
    path.with_file_name(tmp_name)
}

#[cfg(test)]
mod tests {
    use super::{CHECKPOINT_VERSION, DwellingCheckpoint};
    use hares_types::ZoneId;

    fn unique_temp_name(base: &str, ext: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let tid = std::thread::current().id();
        format!("{base}_{nanos}_{tid:?}.{ext}")
    }

    struct TempFile(std::path::PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn save_then_load_round_trip() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 7,
            timestep_index: 12,
            equipment_states: vec![vec![1, 2, 3]],
            rng_state: [42; 32],
            envelope_state: vec![1.0, 2.0],
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![1.0, 2.0, 3.0],
            rng_stream: 3,
            rng_word_pos: 8,
            thermal_last_u: vec![0.1],
            lwr_t_prev_c: vec![15.0, 18.0],
        };

        let path =
            std::env::temp_dir().join(unique_temp_name("hares_core_checkpoint_roundtrip", "json"));
        let _guard = TempFile(path.clone());
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded, cp);
    }

    #[test]
    fn multi_zone_humidity_round_trip() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 42,
            timestep_index: 100,
            equipment_states: vec![],
            rng_state: [0; 32],
            envelope_state: vec![],
            humidity_states: vec![(ZoneId(1), 0.008), (ZoneId(2), 0.012), (ZoneId(3), 0.006)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            thermal_last_u: vec![],
            lwr_t_prev_c: vec![],
        };

        let path =
            std::env::temp_dir().join(unique_temp_name("hares_core_checkpoint_multizone", "json"));
        let _guard = TempFile(path.clone());
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded.humidity_states.len(), 3);
        assert_eq!(loaded.humidity_states[0], (ZoneId(1), 0.008));
        assert_eq!(loaded.humidity_states[1], (ZoneId(2), 0.012));
        assert_eq!(loaded.humidity_states[2], (ZoneId(3), 0.006));
    }

    #[test]
    fn version_mismatch_rejected() {
        let cp = DwellingCheckpoint {
            format_version: 1, // intentionally wrong — older than CHECKPOINT_VERSION
            bldg_id: 1,
            timestep_index: 0,
            equipment_states: vec![],
            rng_state: [0; 32],
            envelope_state: vec![],
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            thermal_last_u: vec![],
            lwr_t_prev_c: vec![],
        };

        let path =
            std::env::temp_dir().join(unique_temp_name("hares_core_checkpoint_version", "json"));
        let _guard = TempFile(path.clone());
        cp.save(&path).unwrap();

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        assert!(
            err.to_string().contains("version mismatch"),
            "expected version mismatch error, got: {err}"
        );
    }

    #[test]
    fn sha256_corruption_rejected() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 1,
            timestep_index: 0,
            equipment_states: vec![],
            rng_state: [0; 32],
            envelope_state: vec![],
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            thermal_last_u: vec![],
            lwr_t_prev_c: vec![],
        };

        let path = std::env::temp_dir().join(unique_temp_name(
            "hares_core_checkpoint_sha256_corrupt",
            "json",
        ));
        let _guard = TempFile(path.clone());
        cp.save(&path).unwrap();

        // Corrupt one byte in the JSON body (after the `sha256:` prefix line)
        let mut file_bytes = std::fs::read(&path).unwrap();
        let nl_pos = file_bytes.iter().position(|&b| b == b'\n').unwrap();
        // Flip a bit in the JSON payload
        if nl_pos + 10 < file_bytes.len() {
            file_bytes[nl_pos + 5] ^= 0x01;
        }
        std::fs::write(&path, &file_bytes).unwrap();

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("SHA-256") || msg.contains("checksum"),
            "expected SHA-256 checksum error, got: {msg}"
        );
    }
}
