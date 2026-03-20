//! Shared text-parsing utilities.

/// Parse a trimmed string as f64.
#[inline]
pub fn parse_trimmed_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Normalize a string: trim whitespace and convert to ASCII lowercase.
#[inline]
pub fn normalize_ascii(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}
