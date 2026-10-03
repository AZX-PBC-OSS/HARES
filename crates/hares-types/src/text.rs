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

/// Classify an HPXML `Location` string as naming conditioned space.
///
/// HPXML 4.2 RefrigeratorLocation enumeration defines the valid location
/// values. This function handles standard HPXML values plus common
/// non-standard strings encountered in field data (e.g. "Indoor",
/// "finished basement").
///
/// Diverges from OCHRE's `parse_zone_name` (ochre/utils/hpxml.py) which
/// classifies `"basement - conditioned"` as `Foundation`, silently zeroing
/// refrigerator gains for conditioned basements. HARES intentionally
/// classifies it as conditioned using substring-keyword matching with
/// correct priority ordering.
#[must_use]
pub fn is_conditioned_location(location: &str) -> bool {
    let s = location.to_ascii_lowercase();
    let s = s.trim();
    // Explicitly unconditioned or unvented spaces are never conditioned.
    if s.contains("uncondition") || s.contains("unvent") {
        return false;
    }
    // Bare garages (but not "garage - conditioned": the "condition" check
    // below catches those).
    if s.contains("garage") && !s.contains("condition") {
        return false;
    }
    // Attics are unconditioned buffer zones.
    if s.contains("attic") {
        return false;
    }
    // Conditioned-space keywords. Handles all HPXML RefrigeratorLocation values
    // that imply a heated/cooled indoor space plus common non-standard strings.
    // HPXML 4.2 data dictionary §RefrigeratorLocation_simple:
    //   "conditioned space", "living space", "kitchen", "other heated space",
    //   "other housing unit", "other non-freezing space", "basement - conditioned",
    //   "garage - conditioned".
    if s.contains("condition")
        || s == "living space"
        || s == "kitchen"
        || s == "indoor"
        || s.contains("heated")
        || s.contains("housing unit")
        || s.contains("non-freezing")
    {
        return true;
    }
    // Finished basements, foundations, and crawlspaces: "finished" implies
    // conditioned in residential building practice. Guard against "unfinished"
    // (which would not reach here because the "uncondition"/"unvent" checks
    // above cover most forms, but bare "unfinished basement" could slip past).
    if (s.contains("basement") || s.contains("foundation") || s.contains("crawl"))
        && s.contains("finished")
        && !s.contains("unfinished")
    {
        return true;
    }
    // HPXML 4.2 RefrigeratorLocation_simple enumeration values that are not
    // conditioned.
    //
    // "other multifamily buffer space": per HPXML 4.2, a semi-conditioned
    // corridor or common area, not a fully conditioned dwelling unit.
    // Treated as non-conditioned (gains zeroed).
    if s == "other multifamily buffer space" {
        return false;
    }
    false
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
