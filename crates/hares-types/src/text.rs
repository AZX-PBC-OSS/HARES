//! Shared text-parsing utilities.

/// Parse a trimmed string as a finite f64.
///
/// `f64::from_str` accepts `"nan"` and `"inf"` as valid parses, so a parse
/// that only checks for `Ok` silently admits values that then poison every
/// downstream computation. Input text carrying a non-finite value is treated
/// as unparseable (`None`), the same as any other malformed number.
#[inline]
pub fn parse_trimmed_f64(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Normalize a string: trim whitespace and convert to ASCII lowercase.
#[inline]
pub fn normalize_ascii(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::parse_trimmed_f64;

    #[test]
    fn parse_trimmed_f64_rejects_non_finite_values() {
        // `f64::from_str` parses all of these as Ok; input carrying them
        // must be treated as unparseable, not as a number.
        for bad in ["nan", "NaN", "inf", "-inf", "infinity", "-Infinity"] {
            assert_eq!(
                parse_trimmed_f64(bad),
                None,
                "parse_trimmed_f64 accepted non-finite '{bad}'"
            );
        }
    }

    #[test]
    fn parse_trimmed_f64_accepts_ordinary_numbers() {
        assert_eq!(parse_trimmed_f64(" 1.5 "), Some(1.5));
        assert_eq!(parse_trimmed_f64("-2e3"), Some(-2000.0));
        assert_eq!(parse_trimmed_f64("0"), Some(0.0));
        assert_eq!(parse_trimmed_f64("abc"), None);
        assert_eq!(parse_trimmed_f64(""), None);
    }
}
