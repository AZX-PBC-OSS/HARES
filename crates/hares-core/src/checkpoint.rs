//! Simulation checkpoint schema and file I/O.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hares_types::{EquipmentId, HaresError};
use serde::{Deserialize, Serialize};

/// Bump whenever checkpoint schema or state encoding changes.
pub const CHECKPOINT_VERSION: u32 = 1;

/// Serializable snapshot of all state required to resume a dwelling run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DwellingCheckpoint {
    pub format_version: u32,
    pub bldg_id: i64,
    pub timestep_index: u64,
    pub equipment_states: Vec<(EquipmentId, Vec<u8>)>,
    pub rng_state: [u8; 32],
    pub envelope_state: Vec<f64>,
    pub humidity_state: f64,
    pub fluid_states: Vec<f64>,
    #[serde(default)]
    pub rng_stream: u64,
    #[serde(default)]
    pub rng_word_pos: u128,
    #[serde(default)]
    pub thermal_last_u: Vec<f64>,
}

impl DwellingCheckpoint {
    /// Persist checkpoint atomically.
    pub fn save(&self, path: &Path) -> Result<(), HaresError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|err| HaresError::Io(format!("checkpoint serialize failed: {err}")))?;
        let tmp_path = temp_checkpoint_path(path);
        fs::write(&tmp_path, bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint temp write failed: {err}")))?;
        fs::rename(&tmp_path, path)
            .map_err(|err| HaresError::Io(format!("checkpoint atomic rename failed: {err}")))?;
        Ok(())
    }

    /// Load checkpoint from disk.
    pub fn load(path: &Path) -> Result<Self, HaresError> {
        let bytes = fs::read(path)
            .map_err(|err| HaresError::Io(format!("checkpoint read failed: {err}")))?;
        let cp: DwellingCheckpoint = serde_json::from_slice(&bytes)
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
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dwelling.chkpt");
    let tmp_name = format!("{file_name}.{nonce}.tmp");
    path.with_file_name(tmp_name)
}

#[cfg(test)]
mod tests {
    use super::{CHECKPOINT_VERSION, DwellingCheckpoint};
    use hares_types::EquipmentId;

    #[test]
    fn save_then_load_round_trip() {
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 7,
            timestep_index: 12,
            equipment_states: vec![(EquipmentId(1), vec![1, 2, 3])],
            rng_state: [42; 32],
            envelope_state: vec![1.0, 2.0],
            humidity_state: 0.005,
            fluid_states: vec![1.0, 2.0, 3.0],
            rng_stream: 3,
            rng_word_pos: 8,
            thermal_last_u: vec![0.1],
        };

        let path = std::env::temp_dir().join("hares_core_checkpoint_roundtrip.json");
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded, cp);
        let _ = std::fs::remove_file(path);
    }
}
