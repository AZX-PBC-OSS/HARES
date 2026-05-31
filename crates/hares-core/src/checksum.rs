//! SHA-256 file integrity checksum helpers.
//!
//! Every persisted file written through these helpers carries a
//! `sha256:<hex>\n` line before the payload so integrity can be verified
//! before any deserialisation attempt.

use sha2::{Digest, Sha256};

/// Literal prefix for the checksum line.  Full line format:
/// `sha256:<64 hex chars>\n`.
const SHA256_PREFIX: &str = "sha256:";

/// Hex-encode byte slice into a lowercase `String`.
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute the SHA-256 hash of `content` and return it as a 64-character
/// hex string.  Useful for diagnostic logging of stored checksums.
#[must_use]
pub fn compute_sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex_encode(&hasher.finalize())
}

/// Prepend a `sha256:<hex>\n` line to `content`, producing the complete
/// self-describing file payload.
#[must_use]
pub fn write_with_sha256(content: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let hash_hex = hex_encode(&hasher.finalize());

    let checksum_line = format!("{SHA256_PREFIX}{hash_hex}\n");
    let mut out = Vec::with_capacity(checksum_line.len() + content.len());
    out.extend_from_slice(checksum_line.as_bytes());
    out.extend_from_slice(content);
    out
}

/// Verify the `sha256:<hex>\n` prefix in `data` against the rest of the
/// slice and return the content (everything after the newline) on success.
///
/// Returns `Err(String)` with a human-readable description on failure so
/// the caller can wrap it in the appropriate domain error.
pub fn verify_sha256(data: &[u8]) -> Result<&[u8], String> {
    let nl_pos = data
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| "missing SHA-256 checksum prefix".to_string())?;

    let prefix_line = std::str::from_utf8(&data[..nl_pos])
        .map_err(|_| "invalid checksum prefix encoding".to_string())?;

    let expected_hex = prefix_line
        .strip_prefix(SHA256_PREFIX)
        .ok_or_else(|| "missing SHA-256 checksum line".to_string())?;

    let content = &data[nl_pos + 1..];

    let mut hasher = Sha256::new();
    hasher.update(content);
    let actual_hex = hex_encode(&hasher.finalize());

    if actual_hex != expected_hex {
        return Err(format!(
            "SHA-256 checksum mismatch: expected {}, computed {}",
            expected_hex, actual_hex
        ));
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let payload = b"{\"hello\":\"world\"}";
        let file_bytes = write_with_sha256(payload);
        let recovered = verify_sha256(&file_bytes).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn detects_corruption() {
        let payload = b"original data";
        let mut file_bytes = write_with_sha256(payload);
        // Flip a byte in the payload
        let content_start = file_bytes.iter().position(|&b| b == b'\n').unwrap() + 1;
        file_bytes[content_start + 3] ^= 0x01;
        assert!(verify_sha256(&file_bytes).is_err());
    }

    #[test]
    fn detects_truncation() {
        let payload = b"original data";
        let mut file_bytes = write_with_sha256(payload);
        // Truncate after the checksum line, removing data
        let nl_pos = file_bytes.iter().position(|&b| b == b'\n').unwrap();
        file_bytes.truncate(nl_pos + 2); // keep only part of the content
        assert!(verify_sha256(&file_bytes).is_err());
    }

    #[test]
    fn hex_encode_round_trip_through_sha256() {
        // Verify hex_encode produces the expected format for a known hash.
        let mut hasher = Sha256::new();
        hasher.update(b"test");
        let hash = hasher.finalize();
        let hex = hex_encode(&hash);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
