//! Simulation checkpoint schema and file I/O.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checksum;
use hares_types::{ElectricalSummary, HaresError, ZoneId};
use serde::{Deserialize, Serialize};

/// Bump whenever checkpoint schema or state encoding changes.
///
/// v7: `actor_states` entries became [`ActorStateCheckpoint`] records
/// carrying a per-actor schema version, and `EvDriverSnapshot` gained the
/// `drive_cancelled` counter in the same change — checkpoints written by
/// v6 builds are rejected by the version gate in [`DwellingCheckpoint::load`].
///
/// v8: `equipment_states` entries became [`EquipmentStateCheckpoint`]
/// records carrying the equipment's name and id, and restore validates
/// both against the live equipment — checkpoints written by v7 builds
/// (positionally-indexed opaque blobs) are rejected by the version gate.
pub const CHECKPOINT_VERSION: u32 = 8;

/// One equipment's checkpointed state, identity-keyed.
///
/// `name` and `equipment_id` are recorded from the equipment's descriptor
/// at save time and both are validated on restore: state is matched to
/// equipment by name, and the id must agree. A spec reorder between save
/// and restore (or any other identity drift) is then a typed error naming
/// both sides — never state silently loading into the wrong equipment,
/// which is what the positional `zip` this record replaced could do (and
/// did, invisibly, for any reordered equipment set).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EquipmentStateCheckpoint {
    /// Equipment instance name, matched against `descriptor().name` on
    /// restore.
    pub name: String,
    /// Equipment id at save time, matched against `descriptor().id` on
    /// restore. Ids are deterministic from spec order, so an id mismatch
    /// is direct evidence the equipment set or its order changed between
    /// save and restore.
    pub equipment_id: u32,
    /// Opaque state blob from `Equipment::save_state()`.
    pub blob: Vec<u8>,
}

/// One actor's checkpointed decision-state.
///
/// `schema_version` is recorded from
/// [`Actor::checkpoint_version`](crate::Actor::checkpoint_version) at save
/// time and validated against the live actor's version on restore, so an
/// actor whose snapshot schema changed is rejected at the checkpoint
/// boundary with a version-mismatch error naming the actor — the actor-side
/// counterpart of the equipment's per-equipment `checkpoint_version()` /
/// `load_versioned` gate (`hares-equipment/src/serial.rs`). Without it, an
/// actor snapshot could gain a field with no gate of its own: the only
/// protection was this file's `format_version`, so an actor-only schema
/// change surfaced as a raw postcard decode failure deep inside the
/// actor's `load_state` instead of as a version mismatch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActorStateCheckpoint {
    /// Actor name, matched against `Actor::name()` on restore.
    pub name: String,
    /// Schema version of `blob`, from `Actor::checkpoint_version()` at save
    /// time. Must equal the live actor's version on restore.
    pub schema_version: u32,
    /// Opaque actor state blob from `Actor::save_state()`. Actors with no
    /// mutable state contribute an empty blob.
    pub blob: Vec<u8>,
}

/// Serializable snapshot of all state required to resume a dwelling run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DwellingCheckpoint {
    pub format_version: u32,
    pub bldg_id: i64,
    pub timestep_index: u64,
    /// Per-equipment state, identity-keyed. Each entry carries the
    /// equipment's name and id alongside its opaque state blob; restore
    /// matches by name and validates the id before handing the blob to
    /// `Equipment::load_state` (see [`EquipmentStateCheckpoint`]).
    pub equipment_states: Vec<EquipmentStateCheckpoint>,
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
    /// Per-zone interior LWR surface temperatures [°C] (ScriptF warm-start).
    /// Outer vec indexed by zone, inner vec by surface.
    pub interior_surface_temps: Vec<Vec<f64>>,
    /// Previous-step per-zone interior LWR surface temperatures [°C]
    /// (heavy-ball damping warm-start). Same shape as `interior_surface_temps`.
    pub interior_surface_prev_temps: Vec<Vec<f64>>,
    /// Actor decision-state, one per registered actor. Each entry carries
    /// the actor's name, its snapshot schema version, and its opaque state
    /// blob; the version is validated on restore before the blob is handed
    /// to the actor (see [`ActorStateCheckpoint`]).
    pub actor_states: Vec<ActorStateCheckpoint>,
    /// Prior-step electrical summary (grid, PV, base load, battery, EV)
    /// so the first post-restore step sees the same env.electrical that
    /// the original continuous run would have at the same step index.
    pub prior_electrical_summary: ElectricalSummary,
}

impl DwellingCheckpoint {
    /// Persist checkpoint atomically with SHA-256 integrity checksum.
    pub fn save(&self, path: &Path) -> Result<(), HaresError> {
        let result = (|| -> Result<(), HaresError> {
            let json_bytes = serde_json::to_vec(self)
                .map_err(|err| HaresError::Io(format!("checkpoint serialize failed: {err}")))?;

            let tmp_bytes = checksum::write_with_sha256(&json_bytes);
            let tmp_path = temp_checkpoint_path(path);
            fs::write(&tmp_path, &tmp_bytes)
                .map_err(|err| HaresError::Io(format!("checkpoint temp write failed: {err}")))?;
            fs::rename(&tmp_path, path)
                .map_err(|err| HaresError::Io(format!("checkpoint atomic rename failed: {err}")))?;
            Ok(())
        })();

        #[cfg(feature = "observe")]
        {
            match &result {
                Ok(()) => tracing::info!(
                    checkpoint_path = %path.display(),
                    "checkpoint saved successfully",
                ),
                Err(e) => tracing::error!(
                    checkpoint_path = %path.display(),
                    error = %e,
                    "checkpoint save failed",
                ),
            }
        }

        result
    }

    /// Deserialize a checkpoint from JSON bytes, gating on
    /// `format_version` before the schema-dependent parse.
    ///
    /// The version probe runs first because a checkpoint written by an
    /// older build may use a schema this build cannot parse (e.g. pre-v7
    /// tuple-shaped `actor_states`), and the actionable error for such a
    /// file is the version mismatch naming both versions — not a parse
    /// failure naming a field the reader cannot act on. Every
    /// deserialisation path (file load, the Python binding's byte-level
    /// save/restore) goes through here so the gate ordering has one home.
    pub fn deserialize_gated(json_bytes: &[u8]) -> Result<Self, HaresError> {
        #[derive(Deserialize)]
        struct FormatProbe {
            format_version: u32,
        }
        let probe: FormatProbe = serde_json::from_slice(json_bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint parse failed: {err}")))?;
        if probe.format_version != CHECKPOINT_VERSION {
            return Err(HaresError::Io(format!(
                "checkpoint version mismatch: file={}, expected={}",
                probe.format_version, CHECKPOINT_VERSION
            )));
        }
        serde_json::from_slice(json_bytes)
            .map_err(|err| HaresError::Io(format!("checkpoint parse failed: {err}")))
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

        Self::deserialize_gated(json_bytes)
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
    use super::{
        ActorStateCheckpoint, CHECKPOINT_VERSION, DwellingCheckpoint, EquipmentStateCheckpoint,
    };
    use hares_types::{ElectricalSummary, ZoneId};

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
            equipment_states: vec![EquipmentStateCheckpoint {
                name: "EV".into(),
                equipment_id: 3,
                blob: vec![1, 2, 3],
            }],
            rng_state: [42; 32],
            envelope_state: vec![1.0, 2.0],
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![1.0, 2.0, 3.0],
            rng_stream: 3,
            rng_word_pos: 8,
            thermal_last_u: vec![0.1],
            lwr_t_prev_c: vec![15.0, 18.0],
            interior_surface_temps: vec![vec![20.0, 21.0], vec![22.0]],
            interior_surface_prev_temps: vec![vec![19.5, 20.5], vec![21.5]],
            actor_states: vec![ActorStateCheckpoint {
                name: "test_actor".into(),
                schema_version: 1,
                blob: vec![1, 2, 3],
            }],
            prior_electrical_summary: ElectricalSummary::default(),
        };

        let path =
            std::env::temp_dir().join(unique_temp_name("hares_core_checkpoint_roundtrip", "json"));
        let _guard = TempFile(path.clone());
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded, cp);
        // The identity fields must survive the round-trip — they are what
        // restore validates against, so a serialization that dropped them
        // would silently regress restore to positional matching.
        assert_eq!(loaded.equipment_states[0].name, "EV");
        assert_eq!(loaded.equipment_states[0].equipment_id, 3);
        assert_eq!(loaded.equipment_states[0].blob, vec![1, 2, 3]);
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
            interior_surface_temps: vec![],
            interior_surface_prev_temps: vec![],
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
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
            interior_surface_temps: vec![],
            interior_surface_prev_temps: vec![],
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
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
    fn old_version_checkpoint_rejected_as_version_mismatch_not_parse_error() {
        // A checkpoint written by a pre-v7 build carries tuple-shaped
        // `actor_states` this build's schema cannot parse. The format-version
        // gate must reject such a file with a version mismatch naming both
        // versions — the actionable error — before any schema-dependent
        // deserialisation, not as a parse failure naming a field the reader
        // cannot act on.
        let body = serde_json::json!({
            "format_version": 6,
            "bldg_id": 1,
            "timestep_index": 0,
            "equipment_states": [],
            "rng_state": vec![0u8; 32],
            "envelope_state": [],
            "humidity_states": [[1, 0.005]],
            "fluid_states": [],
            "rng_stream": 0,
            "rng_word_pos": 0,
            "thermal_last_u": [],
            "lwr_t_prev_c": [],
            "interior_surface_temps": [],
            "interior_surface_prev_temps": [],
            // Pre-v7 shape: (name, blob) tuples, no schema_version.
            "actor_states": [["OldActor", [1, 2, 3]]],
            "prior_electrical_summary": {
                "pv_generation_kw": 0.0,
                "base_load_kw": 0.0,
                "net_grid_kw": 0.0,
                "battery_power_kw": 0.0,
                "ev_power_kw": 0.0,
                "actual_pv_kw": 0.0,
            },
        });
        let json_bytes = serde_json::to_vec(&body).expect("serialize test fixture");
        let file_bytes = crate::checksum::write_with_sha256(&json_bytes);

        let path = std::env::temp_dir().join(unique_temp_name(
            "hares_core_checkpoint_old_version",
            "json",
        ));
        let _guard = TempFile(path.clone());
        std::fs::write(&path, &file_bytes).expect("write test fixture");

        let err = DwellingCheckpoint::load(&path).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint version mismatch")
                && msg.contains("file=6")
                && msg.contains(&format!("expected={CHECKPOINT_VERSION}")),
            "a pre-v7 checkpoint must be rejected by the version gate with both versions named; got: {msg}"
        );
        assert!(
            !msg.contains("parse failed"),
            "the rejection must come from the version gate, not schema parsing; got: {msg}"
        );
    }

    #[test]
    fn deserialize_gated_rejects_malformed_bytes_loudly() {
        // The Python byte-level restore paths call `deserialize_gated`
        // directly on raw caller-supplied bytes, with no SHA-256 pre-check.
        // Bytes that are not JSON at all must produce a typed
        // `checkpoint parse failed` error from the format probe — not a
        // panic and not a silently substituted default.
        let err = DwellingCheckpoint::deserialize_gated(b"this is not json").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint parse failed"),
            "malformed bytes must fail loudly at the probe; got: {msg}"
        );

        // Valid JSON carrying the current format_version but a structurally
        // invalid body must likewise fail at the full parse — not be
        // misreported as a version mismatch (the versions agree).
        let body = serde_json::json!({
            "format_version": CHECKPOINT_VERSION,
            "bldg_id": "not-a-number",
        });
        let err = DwellingCheckpoint::deserialize_gated(
            &serde_json::to_vec(&body).expect("serialize test fixture"),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("checkpoint parse failed"),
            "a matching-version but invalid body must fail at the full parse; got: {msg}"
        );
        assert!(
            !msg.contains("version mismatch"),
            "the versions agree, so the failure must not be reported as a mismatch; got: {msg}"
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
            interior_surface_temps: vec![],
            interior_surface_prev_temps: vec![],
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
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

    #[test]
    fn prior_electrical_summary_round_trip() {
        let summary = ElectricalSummary {
            pv_generation_kw: 5.0,
            base_load_kw: 2.5,
            net_grid_kw: 1.0,
            battery_power_kw: 1.5,
            ev_power_kw: 3.0,
            actual_pv_kw: 4.5,
        };
        let cp = DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: 7,
            timestep_index: 12,
            equipment_states: vec![],
            rng_state: [0; 32],
            envelope_state: vec![],
            humidity_states: vec![(ZoneId(1), 0.005)],
            fluid_states: vec![],
            rng_stream: 0,
            rng_word_pos: 0,
            thermal_last_u: vec![],
            lwr_t_prev_c: vec![],
            interior_surface_temps: vec![],
            interior_surface_prev_temps: vec![],
            actor_states: vec![],
            prior_electrical_summary: summary.clone(),
        };

        let path = std::env::temp_dir().join(unique_temp_name(
            "hares_core_checkpoint_electrical_summary",
            "json",
        ));
        let _guard = TempFile(path.clone());
        cp.save(&path).unwrap();
        let loaded = DwellingCheckpoint::load(&path).unwrap();
        assert_eq!(loaded.prior_electrical_summary, summary);
    }

    #[test]
    fn save_returns_err_on_non_writable_path() {
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
            interior_surface_temps: vec![],
            interior_surface_prev_temps: vec![],
            actor_states: vec![],
            prior_electrical_summary: ElectricalSummary::default(),
        };

        let path = std::path::PathBuf::from("/nonexistent-dir-xyz/cp.json");
        let result = cp.save(&path);
        assert!(
            result.is_err(),
            "save() should return Err for a non-writable path, but returned Ok"
        );
    }
}
