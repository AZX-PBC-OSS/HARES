//! Postcard checkpoint serialization helpers with CRC32 integrity verification.
//!
//! Every blob written through these helpers carries a 4-byte CRC-32/ISCSI
//! suffix appended by `postcard::to_allocvec_crc32`.  On load,
//! `from_bytes_crc32` validates the suffix and returns
//! `Err(PostcardError::CrcMismatch)` on bit-rot corruption.
//!
//! Versioned (`save_versioned` / `load_versioned`) helpers additionally
//! prepend a little-endian `u32` version token so that field-type or
//! enum-variant changes in checkpoint structs are caught with a descriptive
//! error rather than a generic postcard failure.

use hares_types::{EquipmentId, HaresError};
use serde::{Serialize, de::DeserializeOwned};

/// Non-panicking variant for callers that need graceful error handling
/// (e.g., checkpoint paths where a serialization failure should not
/// terminate a long-running simulation).
#[must_use = "serialization errors should be handled, not silently discarded"]
pub fn try_save_postcard<T: Serialize>(state: &T) -> crate::Result<Vec<u8>> {
    let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
    postcard::to_allocvec_crc32(state, crc.digest())
        .map_err(|e| HaresError::Equipment(format!("state serialization failed: {e}")))
}

/// Deserialize checkpoint state via postcard with CRC32 integrity verification.
///
/// On CRC mismatch, `postcard::from_bytes_crc32` returns
/// `Err(PostcardError::CrcMismatch)`.  Callers further up the stack (e.g.
/// `load_versioned`) wrap this with equipment-type and `EquipmentId` context
/// so the error identifies which blob was corrupted.
pub fn load_postcard<T: DeserializeOwned>(bytes: &[u8]) -> crate::Result<T> {
    let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
    postcard::from_bytes_crc32(bytes, crc.digest())
        .map_err(|e| HaresError::Equipment(format!("equipment state deserialization failed: {e}")))
}

// ---------------------------------------------------------------------------
// Versioned postcard checkpoint helpers
// ---------------------------------------------------------------------------

/// Format: `[version: u32 LE][postcard payload]`
const VERSION_PREAMBLE_LEN: usize = std::mem::size_of::<u32>();

/// Serialize state with a per-equipment version token prepended.
///
/// The version is written as a little-endian u32 before the postcard payload.
/// On load, `load_versioned` validates the version before attempting
/// deserialization so that a field-type or enum-variant change in the
/// checkpoint struct is caught with a descriptive error rather than silent
/// corruption or a generic postcard failure.
///
/// # Panics
///
/// Panics on postcard serialization failure. Prefer `try_save_versioned` in
/// production code where panicking would crash a long-running simulation.
pub fn save_versioned<T: Serialize>(state: &T, version: u32, equipment_type: &str) -> Vec<u8> {
    try_save_versioned(state, version, equipment_type)
        .expect("postcard serialization failed in save_versioned — use try_save_versioned for production paths")
}

/// Non-panicking variant of `save_versioned` for production checkpoint paths
/// where a serialization failure must be propagated rather than terminating
/// the process.
///
/// Format: `[version: u32 LE][postcard payload with CRC32 suffix]`
#[must_use = "serialization errors should be handled, not silently discarded"]
pub fn try_save_versioned<T: Serialize>(
    state: &T,
    version: u32,
    equipment_type: &str,
) -> crate::Result<Vec<u8>> {
    #[cfg(feature = "observe")]
    tracing::debug!(
        equipment_type = %equipment_type,
        checkpoint_version = version,
        "saving versioned checkpoint blob",
    );
    #[cfg(not(feature = "observe"))]
    let _ = equipment_type;
    let mut blob = version.to_le_bytes().to_vec();
    blob.extend_from_slice(&try_save_postcard(state)?);
    Ok(blob)
}

/// Load versioned state, validating the version token before deserialization
/// and verifying the CRC32 integrity suffix on the postcard payload.
///
/// Returns an `Err` with equipment type name, expected version, and found
/// version when the blob's version does not match.  On CRC32 mismatch the
/// error includes the equipment type and `EquipmentId` so the failing blob
/// is identifiable.
pub fn load_versioned<T: DeserializeOwned>(
    bytes: &[u8],
    expected_version: u32,
    equipment_type: &str,
    equipment_id: EquipmentId,
) -> crate::Result<T> {
    if bytes.len() < VERSION_PREAMBLE_LEN {
        #[cfg(feature = "observe")]
        tracing::debug!(
            equipment_type = %equipment_type,
            equipment_id = %equipment_id,
            success = false,
            "checkpoint load failed: blob too short for versioned deserialization",
        );
        return Err(HaresError::Equipment(format!(
            "{} checkpoint blob too short for equipment {:?}: {} bytes",
            equipment_type,
            equipment_id,
            bytes.len()
        )));
    }
    // Safety: length checked above — bytes[..4] is guaranteed to have exactly 4 bytes.
    let blob_version = u32::from_le_bytes(bytes[..VERSION_PREAMBLE_LEN].try_into().unwrap());
    if blob_version != expected_version {
        #[cfg(feature = "observe")]
        tracing::debug!(
            equipment_type = %equipment_type,
            equipment_id = %equipment_id,
            blob_version = blob_version,
            expected_version = expected_version,
            success = false,
            "checkpoint load failed: version mismatch",
        );
        return Err(HaresError::Equipment(format!(
            "{} checkpoint version mismatch for equipment {:?}: blob={} expected={}",
            equipment_type, equipment_id, blob_version, expected_version
        )));
    }
    let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);
    let result =
        postcard::from_bytes_crc32(&bytes[VERSION_PREAMBLE_LEN..], crc.digest()).map_err(|e| {
            HaresError::Equipment(format!(
                "{} checkpoint deserialization failed for equipment {:?}: {e}",
                equipment_type, equipment_id
            ))
        });
    #[cfg(feature = "observe")]
    tracing::debug!(
        equipment_type = %equipment_type,
        equipment_id = %equipment_id,
        blob_version = blob_version,
        success = result.is_ok(),
        "checkpoint load complete",
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::{FluidType, LoopId, ProtocolId};
    use serde::{Deserialize, Serialize};

    #[test]
    fn postcard_helpers_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct S {
            a: u32,
            b: f64,
            c: ProtocolId,
            d: LoopId,
            e: FluidType,
        }
        let s = S {
            a: 3,
            b: 4.2,
            c: ProtocolId(7),
            d: LoopId(9),
            e: FluidType::Water,
        };
        let bytes = try_save_postcard(&s).unwrap();
        let decoded: S = load_postcard(&bytes).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn crc32_corruption_detected() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct S {
            x: u32,
            y: f64,
        }
        let s = S { x: 42, y: 3.14 };
        let mut blob = try_save_postcard(&s).unwrap();
        // Corrupt one byte of the postcard blob (skip CRC suffix by
        // corrupting a middle byte — the CRC32 check will catch it)
        blob[10] ^= 0x01;
        let result = load_postcard::<S>(&blob);
        assert!(
            result.is_err(),
            "CRC32 corruption should be detected, but load succeeded"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("CrcMismatch") || err_msg.contains("CRC"),
            "error should mention CRC mismatch; got: {err_msg}"
        );
    }

    #[test]
    fn save_load_versioned_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct S {
            a: u32,
            b: f64,
        }
        let s = S { a: 3, b: 4.2 };
        let bytes = save_versioned(&s, 1, "S");
        let decoded: S = load_versioned(&bytes, 1, "S", EquipmentId(0)).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn version_mismatch_is_rejected_with_descriptive_error() {
        #[derive(Serialize, Deserialize, Debug)]
        struct S {
            a: u32,
        }
        let s = S { a: 42 };
        let mut bytes = save_versioned(&s, 1, "S");
        bytes[0] = 99;
        let result = load_versioned::<S>(&bytes, 1, "S", EquipmentId(7));
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("S") && err_msg.contains("99") && err_msg.contains("1"),
            "error should mention type name, blob version 99, and expected version 1; got: {err_msg}"
        );
    }

    #[test]
    fn versioned_blob_too_short_is_rejected() {
        let result = load_versioned::<u32>(&[1, 2, 3], 1, "T", EquipmentId(0));
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("too short"),
            "error should mention short blob; got: {err_msg}"
        );
    }

    #[test]
    fn save_load_versioned_preserves_postcard_payload() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct P {
            x: u32,
        }
        let p = P { x: 0xDEADBEEF };
        let version: u32 = 1;

        let raw_postcard = try_save_postcard(&p).unwrap();
        let versioned = save_versioned(&p, version, "P");

        assert_eq!(&versioned[..4], &version.to_le_bytes());
        assert_eq!(&versioned[4..], &raw_postcard);

        let decoded: P = load_versioned(&versioned, version, "P", EquipmentId(0)).unwrap();
        assert_eq!(decoded, p);
    }

    #[test]
    fn crc32_in_versioned_blob_detected() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct P {
            x: u32,
        }
        let p = P { x: 0xCAFE };
        let mut blob = save_versioned(&p, 1, "P");
        // Corrupt a byte in the postcard payload portion (past version prefix)
        if blob.len() > 6 {
            blob[6] ^= 0x01;
        }
        let result = load_versioned::<P>(&blob, 1, "P", EquipmentId(99));
        assert!(
            result.is_err(),
            "CRC32 corruption in versioned blob should be detected"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("P") && err_msg.contains("EquipmentId(99)"),
            "error should name equipment type P and id 99; got: {err_msg}"
        );
    }

    #[test]
    fn try_save_postcard_propagates_error_on_custom_serialize_failure() {
        use serde::ser::{self, SerializeStruct};

        struct AlwaysFails;

        impl Serialize for AlwaysFails {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut s = serializer.serialize_struct("AlwaysFails", 1)?;
                s.serialize_field("bad", &FailingField)?;
                s.end()
            }
        }

        struct FailingField;
        impl Serialize for FailingField {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(ser::Error::custom(
                    "deliberate serialization failure for testing",
                ))
            }
        }

        let bad = AlwaysFails;
        let result = try_save_postcard(&bad);
        assert!(
            result.is_err(),
            "try_save_postcard should return Err for types whose Serialize impl fails"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("state serialization failed"),
            "error should mention serialization failure; got: {err_msg}"
        );
    }

    #[test]
    fn try_save_postcard_error_does_not_panic() {
        use serde::ser::Error;

        struct AlwaysFails;
        impl Serialize for AlwaysFails {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(<S as serde::Serializer>::Error::custom("fail"))
            }
        }

        let bad = AlwaysFails;
        // This must not panic — the whole point of try_save_postcard
        let result = try_save_postcard(&bad);
        assert!(result.is_err());
    }
}
