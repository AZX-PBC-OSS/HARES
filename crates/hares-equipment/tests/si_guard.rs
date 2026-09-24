use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

// =============================================================================
// SI Guard — prevent imperial-unit identifiers in equipment simulation code
// =============================================================================
//
// The SI guard ensures that imperial HVAC efficiency units (BTU, ft, cfm, SEER,
// EER, HSPF, COP, etc.) do not appear as identifiers in equipment simulation
// source code.  Imperial units belong only at I/O boundaries (hares-io) where
// HPXML values are converted to SI.
//
// The guard uses `syn` to parse each source file into an AST and walks it to
// collect all identifiers.  This naturally excludes comments and string-literal
// contents — no hand-rolled comment stripper is needed.  Only identifiers that
// appear in executable code are checked.
//
// Per-file allowlists exempt existing files that legitimately contain IP-unit-
// named identifiers (e.g. config structs with SEER/COP fields).
//
// =============================================================================
// Inline annotation allowlist — si-guard-ignore
// =============================================================================
//
// Developers can suppress false-positive IP-unit violations on specific lines
// by adding an inline annotation in a Rust line comment:
//
//   // si-guard-ignore[: reason]
//
// The annotation suppresses IP-unit scanning for:
//   - The line it appears on (same-line suppression).
//   - The immediately following line (preceding-line suppression).
//
// Usage:
//   Same line:
//     let _x = convert_btu(1.0); // si-guard-ignore: EnergyPlus reference value
//   Preceding line:
//     // si-guard-ignore: legacy migration code
//     let _x = convert_btu(1.0);
//
// The `: reason` suffix is optional but recommended for auditability.
// Suppressed violations are counted and reported in the test output so
// reviewers can audit whether annotations are being overused.

/// A forbidden imperial-unit pattern with its per-file allowlist.
struct Rule {
    re: Regex,
    name: &'static str,
    allowed: &'static [&'static str],
}

fn rules() -> Vec<Rule> {
    vec![
        // --- BTU / Btu: standalone, case-insensitive (not inside identifiers) ---
        Rule {
            re: Regex::new(r"(?i)\bBTU\b").unwrap(),
            name: "BTU / Btu",
            allowed: &[
                "generator.rs",
                "scheduled_load.rs",
                "water_heater/gas.rs",
                "water_heater/mod.rs",
                "water_heater/resistance.rs",
                "hvac/air_conditioner.rs",
                "hvac/cooling_config.rs",
                "hvac/heating_config.rs",
                "hvac/heat_pump/heater.rs",
            ],
        },
        // --- ft: feet (as standalone word, not e.g. "left" or "after") ---
        Rule {
            re: Regex::new(r"\bft\b").unwrap(),
            name: "ft (feet)",
            allowed: &[
                "hvac/air_conditioner.rs",
                "hvac/heat_pump_config.rs",
                "ventilation.rs",
                "water_heater/indirect_tank.rs",
                "water_heater/mod.rs",
                "water_heater/resistance.rs",
                "water_heater/wh_config.rs",
            ],
        },
        // --- cfm: cubic feet per minute ---
        // Uses (?:\b|_) boundaries so _cfm and cfm_ in identifiers are caught
        // (e.g. fan_power_w_per_cfm, CFM_PER_M3_S) while still requiring a word
        // or underscore boundary on each side to avoid partial substring matches.
        Rule {
            re: Regex::new(r"(?i)(?:\b|_)cfm(?:\b|_)").unwrap(),
            name: "cfm",
            allowed: &[
                "ventilation.rs",
                "hvac/hvac_core.rs",
                "hvac/cooling_config.rs",
                "hvac/heat_pump_config.rs",
                "hvac/air_conditioner.rs",
                "hvac/heat_pump/cooler.rs",
                "hvac/heat_pump/heater.rs",
                "hvac/coil_physics.rs",
            ],
        },
        // --- gpm: gallons per minute ---
        Rule {
            re: Regex::new(r"\bgpm\b").unwrap(),
            name: "gpm",
            allowed: &[
                "hvac/heating_config.rs",
                "hvac/heat_pump_config.rs",
                "water_heater/wh_config.rs",
            ],
        },
        // --- gallon: full word US gallon ---
        Rule {
            re: Regex::new(r"\bgallon\b").unwrap(),
            name: "gallon",
            allowed: &["ev/catalog.rs", "water_heater/indirect_tank.rs"],
        },
        // --- gal: gallon abbreviation ---
        Rule {
            re: Regex::new(r"\bgal\b").unwrap(),
            name: "gal",
            allowed: &["water_heater/indirect_tank.rs", "water_heater/mod.rs"],
        },
        // --- therm: US therm of natural gas ---
        Rule {
            re: Regex::new(r"\btherm\b").unwrap(),
            name: "therm",
            allowed: &["scheduled_load.rs", "water_heater/gas.rs"],
        },
        // --- inch / inches: imperial inch (full word and plural) ---
        Rule {
            re: Regex::new(r"\binch(?:es)?\b").unwrap(),
            name: "inch / inches",
            allowed: &["hvac/heat_pump/heater.rs"],
        },
        // --- lb: pound-mass ---
        Rule {
            re: Regex::new(r"\blbs?\b").unwrap(),
            name: "lb",
            allowed: &[],
        },
        // --- mph: miles per hour ---
        Rule {
            re: Regex::new(r"\bmph\b").unwrap(),
            name: "mph",
            allowed: &[],
        },
        // --- psf / psi: pounds per square foot / inch ---
        Rule {
            re: Regex::new(r"\bps[fi]\b").unwrap(),
            name: "psf/psi",
            allowed: &[],
        },
        // --- Fahrenheit: the full word ---
        Rule {
            re: Regex::new(r"\bFahrenheit\b").unwrap(),
            name: "Fahrenheit",
            allowed: &["water_heater/hpwh_compressor.rs"],
        },
        // --- pound / pounds: pound-mass (full word, distinct from lb/lbs) ---
        Rule {
            re: Regex::new(r"\bpounds?\b").unwrap(),
            name: "pound / pounds",
            allowed: &[],
        },
        // --- feet: feet (full word, distinct from ft) ---
        Rule {
            re: Regex::new(r"\bfeet\b").unwrap(),
            name: "feet",
            allowed: &[],
        },
        // --- SEER: Seasonal Energy Efficiency Ratio (cooling) ---
        Rule {
            re: Regex::new(r"\bSEER\b").unwrap(),
            name: "SEER",
            allowed: &[
                "hvac/air_conditioner.rs",
                "hvac/cooling_config.rs",
                "hvac/core_config.rs",
                "hvac/heat_pump/heater.rs",
                "hvac/hvac_core.rs",
            ],
        },
        // --- EER: Energy Efficiency Ratio (standalone, not inside SEER) ---
        Rule {
            re: Regex::new(r"\bEER\b").unwrap(),
            name: "EER",
            allowed: &["hvac/default_curves.rs"],
        },
        // --- HSPF: Heating Seasonal Performance Factor ---
        Rule {
            re: Regex::new(r"\bHSPF\b").unwrap(),
            name: "HSPF",
            allowed: &["hvac/core_config.rs", "hvac/hvac_core.rs"],
        },
        // --- COP: Coefficient of Performance (used as IP-derived metric alongside
        // SEER / EER / HSPF in equipment efficiency rating contexts) ---
        Rule {
            re: Regex::new(r"\bCOP\b").unwrap(),
            name: "COP",
            allowed: &[
                "hvac/ac_config.rs",
                "hvac/air_conditioner.rs",
                "hvac/cooling_config.rs",
                "hvac/default_curves.rs",
                "hvac/heat_pump/heater.rs",
                "hvac/heat_pump/heater_config.rs",
                "hvac/heat_pump_config.rs",
                "hvac/heating_config.rs",
                "hvac/hvac_core.rs",
                "hvac/ideal_hvac.rs",
                "hvac/staging.rs",
                "ndinterp.rs",
                "water_heater/heat_pump_wh.rs",
                "water_heater/hpwh_compressor.rs",
            ],
        },
    ]
}

fn collect_rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("read_dir");
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// AST visitor that collects all identifiers from parsed Rust source.
#[derive(Default)]
struct IdentifierCollector {
    identifiers: HashSet<String>,
}

impl<'ast> Visit<'ast> for IdentifierCollector {
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        self.identifiers.insert(ident.to_string());
        syn::visit::visit_ident(self, ident);
    }
}

/// Returns true if `si-guard-ignore` appears in an actual Rust comment (`//`
/// or `/* */`) anywhere on `line`, excluding occurrences inside string or char
/// literals.
///
/// `in_block_comment` tracks whether we entered a `/*` on a prior line that has
/// not yet been closed by `*/`.  It is updated in place.
fn line_has_annotation_in_comment(line: &str, in_block_comment: &mut bool) -> bool {
    if !line.contains("si-guard-ignore") {
        return false;
    }

    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Multi-line block comment opened on a previous line.
        if *in_block_comment {
            if let Some(end_pos) = line[i..].find("*/") {
                let comment_part = &line[i..i + end_pos];
                if comment_part.contains("si-guard-ignore") {
                    return true;
                }
                i += end_pos + 2;
                *in_block_comment = false;
                continue;
            } else {
                return line[i..].contains("si-guard-ignore");
            }
        }

        // Skip string literal — tracks quoted `"..."` and raw `r#"..."#`
        // spans by walking from opening `"` to its closing `"`, skipping `\"`
        // escape pairs.  Raw strings happen to work because their body is
        // between matching `"` chars and the fixtures contain no embedded
        // unescaped `"`.  Full hash-delimiter awareness (e.g. `r##"..."##`)
        // is not implemented — the parser-based guard already excludes string
        // content from violation scanning, so this scanner only needs to keep
        // the per-line annotation scan correct.
        if bytes[i] == b'"' {
            i += 1;
            while i < len {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }

        // Skip char literal or lifetime.
        // A Rust `'` opens either a char literal (`'a'`, `'\n'`, `'\''`)
        // which has a closing `'`, or a lifetime (`'a`, `'static`, `'_`,
        // `'long_name`) which does not.  Disambiguating avoids the scanner
        // consuming the entire rest of the line chasing a closing `'` that
        // will never arrive, silently swallowing any `// si-guard-ignore`
        // comment that follows.
        if bytes[i] == b'\'' {
            i += 1;
            if i >= len {
                continue;
            }

            if bytes[i] == b'\\' {
                // Escape sequence: char literal like '\n', '\u{...}', '\''
                i += 1; // skip backslash
                if i < len {
                    if bytes[i] == b'u' {
                        // Unicode escape: \u{...}
                        while i < len && bytes[i] != b'\'' {
                            i += 1;
                        }
                        if i < len {
                            i += 1; // skip closing '
                        }
                    } else {
                        // Simple escape: \n, \\, \t, \r, \', \"
                        i += 1; // skip escaped char
                        if i < len && bytes[i] == b'\'' {
                            i += 1; // skip closing '
                        }
                    }
                }
            } else if i + 1 < len && bytes[i + 1] == b'\'' {
                // Single-char literal: 'a', '1', '_', '"' etc.
                i += 2; // skip char + closing '
            } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
                // Lifetime: 'a, 'static, '_, 'long_name — no closing '
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
            } else {
                // Neither — some edge case (broken code, lone ' in comment).
                // Fall back to scanning to next ' or end-of-line.
                while i < len && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < len {
                    i += 1; // skip '
                }
            }
            continue;
        }

        // Line comment `//` — the rest of the line is comment text.
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            return line[i..].contains("si-guard-ignore");
        }

        // Block comment `/* ... */`.
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            if let Some(end_pos) = line[i..].find("*/") {
                let comment_part = &line[i..i + end_pos];
                if comment_part.contains("si-guard-ignore") {
                    return true;
                }
                i += end_pos + 2;
            } else {
                *in_block_comment = true;
                return line[i..].contains("si-guard-ignore");
            }
            continue;
        }

        i += 1;
    }

    false
}

/// Returns true if the current raw line should be suppressed from IP-unit
/// scanning because it (or the immediately preceding line) contains a
/// `si-guard-ignore` annotation inside a Rust comment.
///
/// `prev_has_ignore` is updated in place to reflect whether this line contains
/// an annotation, for use by the next call.
/// `in_block_comment` tracks `/* */` block comment state across lines.
fn annotation_suppresses_line(
    raw_line: &str,
    prev_has_ignore: &mut bool,
    in_block_comment: &mut bool,
) -> bool {
    let current_has_ignore = line_has_annotation_in_comment(raw_line, in_block_comment);
    let suppress = current_has_ignore || *prev_has_ignore;
    *prev_has_ignore = current_has_ignore;
    suppress
}

struct ScanResult {
    violations: Vec<String>,
    suppressed_count: usize,
}

/// Scans `source` for IP-unit violations using `syn` to distinguish code
/// identifiers from comments and string literals.
///
/// `rel` is the file-relative path used for allowlist matching (empty string
/// for unit tests where no allowlist applies).
fn scan_source(source: &str, rel: &str, all_rules: &[Rule]) -> ScanResult {
    let file = match syn::parse_file(source) {
        Ok(f) => f,
        Err(_) => {
            return ScanResult {
                violations: vec![],
                suppressed_count: 0,
            };
        }
    };

    let mut collector = IdentifierCollector::default();
    syn::visit::visit_file(&mut collector, &file);

    // Pre-compute which rules have matching AST identifiers.  If a rule's
    // regex does not match any identifier in the AST, no line can produce a
    // code-level violation for that rule — any raw-line match would be from
    // a comment or string literal, which the parser excludes.
    let active_rules: Vec<&Rule> = all_rules
        .iter()
        .filter(|rule| {
            if rule.allowed.contains(&rel) {
                return false;
            }
            collector
                .identifiers
                .iter()
                .any(|ident| rule.re.is_match(ident))
        })
        .collect();

    if active_rules.is_empty() {
        return ScanResult {
            violations: vec![],
            suppressed_count: 0,
        };
    }

    let mut violations = Vec::new();
    let mut suppressed_count = 0;
    let mut prev_has_ignore = false;
    let mut in_block_comment = false;

    for (line_idx, raw_line) in source.lines().enumerate() {
        let is_suppressed =
            annotation_suppresses_line(raw_line, &mut prev_has_ignore, &mut in_block_comment);

        for rule in &active_rules {
            if rule.re.is_match(raw_line) {
                if is_suppressed {
                    suppressed_count += 1;
                } else {
                    violations.push(format!(
                        "::error file={rel},line={}::forbidden imperial marker '{}' (pattern: {})",
                        line_idx + 1,
                        rule.name,
                        rule.re.as_str()
                    ));
                }
            }
        }
    }

    ScanResult {
        violations,
        suppressed_count,
    }
}

/// Returns true if `source` contains any non-suppressed IP-unit violation.
fn has_violations_in_source(source: &str, rules: &[Rule]) -> bool {
    !scan_source(source, "", rules).violations.is_empty()
}

#[test]
fn no_new_imperial_conversion_markers_in_equipment_src() {
    // Guard intent:
    // - conversions between imperial HVAC efficiency units and SI must live in
    //   IO/periphery, not equipment simulation code.
    // - this test uses `syn` to parse each source file and walks the AST to
    //   collect identifiers.  Comments and string-literal contents are
    //   naturally excluded — no hand-rolled comment stripper is needed.
    // - per-pattern file allowlists exempt files that currently contain
    //   legitimate IP-unit-named identifiers.
    // - developers may suppress specific lines with a
    //   `// si-guard-ignore[: reason]` annotation; suppressed matches are
    //   counted and reported separately.
    // - new imperial identifiers in non-allowlisted code are caught.

    let rules = rules();
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_root, &mut files);

    let mut violations = Vec::new();
    let mut suppressed_count: usize = 0;
    for file in &files {
        let rel = file
            .strip_prefix(&src_root)
            .expect("strip prefix")
            .to_string_lossy()
            .replace('\\', "/");
        let content = fs::read_to_string(file).expect("read source file");
        let result = scan_source(&content, &rel, &rules);
        violations.extend(result.violations);
        suppressed_count += result.suppressed_count;
    }

    if suppressed_count > 0 || !violations.is_empty() {
        eprintln!(
            "SI guard: {} violation(s), {} suppressed by annotation",
            violations.len(),
            suppressed_count
        );
    }

    assert!(
        violations.is_empty(),
        "SI guard failed:\n{}",
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Unit tests — verify regex pattern correctness independent of source scan
// ---------------------------------------------------------------------------

#[test]
fn btu_matches_unit_reference_not_symbol_name() {
    let re = Regex::new(r"\bBTU\b").unwrap();
    // Standalone BTU in comments or docs.
    assert!(re.is_match("convert from BTU to J"));
    assert!(re.is_match("180,000 BTU/hr"));
    // Inside underscored identifier: no word boundary after U → no match.
    assert!(!re.is_match("BTU_PER_HR_PER_W"));
    assert!(!re.is_match("BTU_IT"));
}

#[test]
fn ft_matches_unit_not_substring() {
    let re = Regex::new(r"\bft\b").unwrap();
    // Standalone at word boundaries.
    assert!(re.is_match("10 ft"));
    assert!(re.is_match("10.0 ft²"));
    // Inside words: no word boundary → no match.
    assert!(!re.is_match("left"));
    assert!(!re.is_match("after"));
    assert!(!re.is_match("shift"));
    assert!(!re.is_match("craft"));
    assert!(!re.is_match("software"));
}

#[test]
fn cfm_matches_standalone_and_in_identifier() {
    let re = Regex::new(r"(?i)(?:\b|_)cfm(?:\b|_)").unwrap();
    // Standalone with surrounding non-word characters, any case.
    assert!(re.is_match("0.365 W/CFM"));
    assert!(re.is_match("350 CFM/ton"));
    assert!(re.is_match("75 cfm ventilation"));
    // Inside snake_case identifiers — caught via underscore boundary.
    assert!(re.is_match("fan_power_w_per_cfm"));
    assert!(re.is_match("CFM_PER_M3_S"));
    assert!(re.is_match("DEFAULT_FAN_POWER_W_PER_CFM"));
}

#[test]
fn inch_matches_unit_not_substring() {
    let re = Regex::new(r"\binch\b").unwrap();
    assert!(re.is_match("1.5 inch pipe"));
    // Not substrings of larger words.
    assert!(!re.is_match("intake"));
    assert!(!re.is_match("inside"));
    assert!(!re.is_match("pinch"));
}

#[test]
fn gpm_gal_gallon_match_standalone() {
    let gpm = Regex::new(r"\bgpm\b").unwrap();
    assert!(gpm.is_match("flow rate 8 gpm"));
    assert!(!gpm.is_match("egpm")); // not a standalone word

    let gal = Regex::new(r"\bgal\b").unwrap();
    assert!(gal.is_match("50 gal tank"));
    assert!(!gal.is_match("legal")); // not standalone
    assert!(!gal.is_match("gallon")); // different word

    let gallon = Regex::new(r"\bgallon\b").unwrap();
    assert!(gallon.is_match("per gallon gasoline"));
    assert!(!gallon.is_match("gallons")); // plural
}

#[test]
fn lb_and_mph_match_standalone() {
    let lb = Regex::new(r"\blbs?\b").unwrap();
    assert!(lb.is_match("5 lb"));
    assert!(lb.is_match("10 lbs"));
    assert!(!lb.is_match("bulb")); // not standalone

    let mph = Regex::new(r"\bmph\b").unwrap();
    assert!(mph.is_match("60 mph"));
    assert!(!mph.is_match("mphm")); // not standalone
}

#[test]
fn psf_psi_match_standalone() {
    let re = Regex::new(r"\bps[fi]\b").unwrap();
    assert!(re.is_match("20 psf load"));
    assert!(re.is_match("15 psi pressure"));
    assert!(!re.is_match("pseudo"));
    assert!(!re.is_match("tips"));
}

#[test]
fn therm_matches_standalone() {
    let re = Regex::new(r"\btherm\b").unwrap();
    assert!(re.is_match("1 US therm = 100,000 BTU"));
    assert!(!re.is_match("isothermal")); // not standalone
    assert!(!re.is_match("thermistor"));
}

// ---------------------------------------------------------------------------
// Regression test — T-0360: verify old dead markers are gone and new
// patterns catch the live IP-unit references from Finding 3.
// ---------------------------------------------------------------------------

#[test]
fn regression_old_markers_gone_new_patterns_catch_finding_3_refs() {
    let rules = rules();

    // Part 1: old dead markers (T-0360 / Finding 2) are not matched
    // by any regex pattern in the current rules list.
    let old_markers = [
        "BTU_PER_HR_PER_W",
        "HeatingEfficiencyUnit::Seer",
        "HeatingEfficiencyUnit::Eer",
        "HeatingEfficiencyUnit::Hspf",
    ];
    for marker in &old_markers {
        for rule in &rules {
            assert!(
                !rule.re.is_match(marker),
                "old dead marker '{}' was matched by pattern '{}' ({}) — should not be in the marker list",
                marker,
                rule.name,
                rule.re.as_str()
            );
        }
    }

    // Part 2: the new regex patterns catch the real IP-unit references
    // from Finding 3 (water_heater comments, scheduled_load therm,
    // heat_pump_config cfm).
    let ft_re = &rules
        .iter()
        .find(|r| r.name == "ft (feet)")
        .expect("ft pattern exists")
        .re;
    assert!(ft_re.is_match("10 hr·ft²·°F/BTU"));
    assert!(ft_re.is_match("— 10 hr·ft²·°F/BTU -- a"));

    let btu_re = &rules
        .iter()
        .find(|r| r.name == "BTU / Btu")
        .expect("BTU / Btu pattern exists")
        .re;
    assert!(btu_re.is_match("10 hr·ft²·°F/BTU"));
    assert!(btu_re.is_match("10 000 Btu/h"));

    let therm_re = &rules
        .iter()
        .find(|r| r.name == "therm")
        .expect("therm pattern exists")
        .re;
    assert!(therm_re.is_match("1 US therm = 100,000 n_IT"));

    let cfm_re = &rules
        .iter()
        .find(|r| r.name == "cfm")
        .expect("cfm pattern exists")
        .re;
    assert!(cfm_re.is_match("fan_power_w_per_cfm"));

    // Part 3 (T-0361): IP-derived efficiency metrics are caught.
    let seer_re = &rules
        .iter()
        .find(|r| r.name == "SEER")
        .expect("SEER pattern exists")
        .re;
    assert!(seer_re.is_match("SEER 14 rating"));
    assert!(!seer_re.is_match("SEER2")); // not standalone

    let eer_re = &rules
        .iter()
        .find(|r| r.name == "EER")
        .expect("EER pattern exists")
        .re;
    assert!(eer_re.is_match("EER 12 rating"));
    // \bEER\b does NOT match inside "SEER" because no word boundary before E
    assert!(!eer_re.is_match("SEER 14"));

    let hspf_re = &rules
        .iter()
        .find(|r| r.name == "HSPF")
        .expect("HSPF pattern exists")
        .re;
    assert!(hspf_re.is_match("HSPF 8.5 rating"));
    assert!(!hspf_re.is_match("HSPF2"));

    let cop_re = &rules
        .iter()
        .find(|r| r.name == "COP")
        .expect("COP pattern exists")
        .re;
    assert!(cop_re.is_match("COP 3.5"));
    assert!(!cop_re.is_match("COPPER"));
}

// ---------------------------------------------------------------------------
// T-0361: Marker coverage tests
// ---------------------------------------------------------------------------

#[test]
fn marker_list_meets_minimum_coverage() {
    let all = rules();
    assert!(
        all.len() >= 13,
        "marker list length {} is below minimum 13; add more IP-unit patterns",
        all.len()
    );
}

#[test]
fn each_marker_has_word_boundary_anchors() {
    let all = rules();
    for rule in &all {
        let pattern = rule.re.as_str();
        assert!(
            pattern.contains(r"\b"),
            "rule '{}' pattern '{}' lacks \\b word-boundary anchors",
            rule.name,
            pattern
        );
    }
}

#[test]
fn fahrenheit_matches_full_word_not_substring() {
    let re = Regex::new(r"\bFahrenheit\b").unwrap();
    assert!(re.is_match("Fahrenheit scale"));
    assert!(re.is_match("Source values in Fahrenheit: lower = 45°F"));
    assert!(!re.is_match("Fahrenheiting")); // not standalone
    assert!(!re.is_match("Fahrenheits")); // plural without boundary
}

#[test]
fn pound_matches_standalone_not_compound_substring() {
    let re = Regex::new(r"\bpounds?\b").unwrap();
    assert!(re.is_match("5 pound mass"));
    assert!(re.is_match("10 pounds weight"));
    // "compound" / "compounds" / "compounding": no word boundary before "pound"
    assert!(!re.is_match("compound"));
    assert!(!re.is_match("compounds"));
    assert!(!re.is_match("compounding"));
    assert!(!re.is_match("pounding")); // word-internal
}

#[test]
fn feet_matches_standalone_not_substring() {
    let re = Regex::new(r"\bfeet\b").unwrap();
    assert!(re.is_match("10 feet long"));
    assert!(re.is_match("height in feet"));
    assert!(!re.is_match("feets")); // non-English
    assert!(!re.is_match("feete")); // non-word
}

#[test]
fn seer_eer_hspf_match_standalone_not_wider_word() {
    let seer = Regex::new(r"\bSEER\b").unwrap();
    assert!(seer.is_match("SEER 14 rating"));
    assert!(!seer.is_match("SEER2")); // no word boundary after R

    // \bEER\b does NOT match inside "SEER" — no word boundary before E
    let eer = Regex::new(r"\bEER\b").unwrap();
    assert!(eer.is_match("EER 12 rating"));
    assert!(!eer.is_match("SEER 14"));
    assert!(!eer.is_match("EERR")); // not standalone

    let hspf = Regex::new(r"\bHSPF\b").unwrap();
    assert!(hspf.is_match("HSPF 8.5"));
    assert!(!hspf.is_match("HSPF2")); // no word boundary after F
}

#[test]
fn cop_matches_standalone_case_sensitive() {
    let re = Regex::new(r"\bCOP\b").unwrap();
    assert!(re.is_match("COP 3.5"));
    assert!(re.is_match("heating COP rating"));
    assert!(!re.is_match("cop")); // case-sensitive
    assert!(!re.is_match("COPPER")); // not standalone
    assert!(!re.is_match("copper")); // not standalone
}

#[test]
fn known_ip_unit_references_are_flagged() {
    let all = rules();
    let ip_refs: Vec<(&str, &[&str])> = vec![
        ("10 hr·ft²·°F/BTU", &["ft (feet)", "BTU / Btu"]),
        ("1 US therm = 100,000 n_IT", &["therm"]),
        ("fan_power_w_per_cfm", &["cfm"]),
        ("SEER 14 cooling efficiency", &["SEER"]),
        ("EER 12 cooling efficiency", &["EER"]),
        ("HSPF 8.5 heating performance", &["HSPF"]),
        ("COP 3.5 heat pump efficiency", &["COP"]),
        ("Fahrenheit temperature scale", &["Fahrenheit"]),
        ("5 lb mass", &["lb"]),
        ("10 lbs weight", &["lb"]),
        ("1.5 inch pipe diameter", &["inch / inches"]),
        ("3 inches diameter", &["inch / inches"]),
        ("60 mph wind speed", &["mph"]),
        ("20 psf load", &["psf/psi"]),
        ("15 psi pressure", &["psf/psi"]),
        ("50 gallon tank", &["gallon"]),
        ("40 gal water heater", &["gal"]),
        ("5 pound mass", &["pound / pounds"]),
    ];

    for (text, expected_names) in &ip_refs {
        let matched: Vec<&str> = all
            .iter()
            .filter(|r| r.re.is_match(text))
            .map(|r| r.name)
            .collect();
        for expected in *expected_names {
            assert!(
                matched.contains(expected),
                "rule '{}' did not match IP reference '{}'; matched rules: {:?}",
                expected,
                text,
                matched
            );
        }
    }
}

#[test]
fn si_only_units_produce_zero_false_positives() {
    let all = rules();
    let si_texts = &[
        "Power: 3500 W, Energy: 12600000 J",
        "Length: 3.5 m, Mass: 2.3 kg, Temperature: 293.15 K",
        "Pressure: 101325 Pa, Volume: 1.0 m³",
        "Density: 1.204 kg/m³, Velocity: 5.0 m/s",
        "Specific heat: 1005 J/(kg·K), Conductivity: 0.026 W/(m·K)",
    ];
    for text in si_texts {
        for rule in &all {
            assert!(
                !rule.re.is_match(text),
                "rule '{}' pattern '{}' falsely matched SI-only text: '{}'",
                rule.name,
                rule.re.as_str(),
                text
            );
        }
    }
}

// ---------------------------------------------------------------------------
// T-0362 (revised): syn-based comment and string-literal exclusion tests
// ---------------------------------------------------------------------------
//
// The guard now uses `syn` to parse source files and walk the AST for
// identifiers.  Comments and string-literal contents are naturally excluded
// because they are not identifiers in the AST.  These tests verify that
// behaviour end-to-end through `has_violations_in_source`.

#[test]
fn ip_unit_in_line_comment_not_flagged() {
    let rules = rules();
    let source = "fn main() {\n    // convert from BTU to J\n    let _x = 1.0;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "BTU in a line comment should not be flagged"
    );
}

#[test]
fn ip_unit_in_block_comment_not_flagged() {
    let rules = rules();
    let source = "fn main() {\n    /* 180,000 BTU/hr */\n    let _x = 1.0;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "BTU in a block comment should not be flagged"
    );
}

#[test]
fn ip_unit_in_nested_block_comment_not_flagged() {
    let rules = rules();
    let source = "fn main() {\n    /* outer /* inner */ still outer BTU */\n    let _x = 1.0;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "BTU in a nested block comment should not be flagged"
    );
}

#[test]
fn ip_unit_in_string_literal_not_flagged() {
    let rules = rules();
    let source = "fn main() {\n    let msg = \"temperature in Fahrenheit\";\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "Fahrenheit in a string literal should not be flagged"
    );
}

#[test]
fn ip_unit_in_raw_string_literal_not_flagged() {
    let rules = rules();
    let source = "fn main() {\n    let src = r#\"BTU per hour\"#;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "BTU in a raw string literal should not be flagged"
    );
}

#[test]
fn ip_unit_as_identifier_flagged() {
    let rules = rules();
    let source = "fn main() {\n    let btu = 1.0;\n}\n";
    assert!(
        has_violations_in_source(source, &rules),
        "btu as a code identifier must be flagged"
    );
}

#[test]
fn ip_unit_in_camelcase_identifier_not_flagged_when_no_boundary() {
    // `convertBtu` — \bBTU\b should NOT match because `B` is preceded by `t`
    // (word char) and `u` is followed by nothing (end of ident).  But `syn`
    // treats `convertBtu` as a single identifier, and the regex \bBTU\b
    // applied to the string `convertBtu` has no word boundary before `B`.
    // So this is correctly NOT flagged.
    let rules = rules();
    let source = "fn main() {\n    let _x = convertBtu(1.0);\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "BTU inside a camelCase identifier should not be flagged (no word boundary)"
    );
}

#[test]
fn ip_unit_in_snake_case_identifier_caught_by_underscore_rule() {
    // cfm rule uses (?:\b|_) boundaries — `fan_power_w_per_cfm` is caught.
    let rules = rules();
    let source = "fn main() {\n    let fan_power_w_per_cfm = 0.365;\n}\n";
    assert!(
        has_violations_in_source(source, &rules),
        "cfm inside a snake_case identifier must be flagged (underscore boundary)"
    );
}

// ---------------------------------------------------------------------------
// T-0363: Inline annotation allowlist — si-guard-ignore
// ---------------------------------------------------------------------------

#[test]
fn si_guard_ignore_same_line_suppresses_violation() {
    let rules = rules();
    let source = "fn main() {\n    let btu = 1.0; // si-guard-ignore: legacy migration code\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "same-line si-guard-ignore annotation should suppress the BTU violation"
    );
}

#[test]
fn si_guard_ignore_in_block_comment_on_preceding_line_suppresses_violation() {
    let rules = rules();
    let source = "fn main() {\n    /* si-guard-ignore: preceding */\n    let btu = 1.0;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore in a /* */ block comment on the preceding line should suppress the violation"
    );
}

#[test]
fn si_guard_ignore_absent_flags_violation() {
    let rules = rules();
    let source = "fn main() {\n    let btu = 1.0;\n}\n";
    assert!(
        has_violations_in_source(source, &rules),
        "no si-guard-ignore annotation should allow the BTU violation to be flagged"
    );
}

#[test]
fn si_guard_ignore_annotation_does_not_cause_false_positive() {
    // The annotation text "si-guard-ignore" contains no IP-unit keywords.
    // Verify that adding the annotation does not introduce any false violations.
    let rules = rules();
    let source =
        "fn main() {\n    // si-guard-ignore: legacy reference value\n    let _x = 1.0;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore annotation should not introduce any false positives"
    );
}

#[test]
fn si_guard_ignore_suppressed_count_reported() {
    let rules = rules();
    // The annotation on the `btu` line suppresses that line AND the
    // immediately following line.  `therm` is placed two lines below so
    // it is NOT suppressed and remains a violation.
    let source = "fn main() {\n    let btu = 1.0; // si-guard-ignore: legacy\n    let _gap = 0.0;\n    let therm = 2.0;\n}\n";
    let result = scan_source(source, "", &rules);
    assert_eq!(
        result.suppressed_count, 1,
        "one violation should be suppressed by annotation"
    );
    assert_eq!(
        result.violations.len(),
        1,
        "one violation (therm) should remain unsuppressed"
    );
}

#[test]
fn si_guard_ignore_in_string_literal_does_not_suppress() {
    let rules = rules();
    // `si-guard-ignore` inside a string literal must not suppress violations.
    let source = "fn main() {\n    let label = \"si-guard-ignore\"; let btu = 1.0;\n}\n";
    assert!(
        has_violations_in_source(source, &rules),
        "si-guard-ignore in a string literal should NOT suppress violations"
    );
}

#[test]
fn si_guard_ignore_in_string_does_not_count_as_suppressed() {
    let rules = rules();
    let source = "fn main() {\n    let msg = \"si-guard-ignore\"; let btu = 1.0;\n}\n";
    let result = scan_source(source, "", &rules);
    assert_eq!(
        result.suppressed_count, 0,
        "annotation in string literal should not count as suppressed"
    );
    assert_eq!(
        result.violations.len(),
        1,
        "btu violation should be flagged (not suppressed by string content)"
    );
}

#[test]
fn si_guard_ignore_in_line_comment_syntax_inside_string_does_not_suppress() {
    let rules = rules();
    // `// si-guard-ignore` inside a string literal is not a real comment.
    let source = "fn main() {\n    let _note = \"// si-guard-ignore\"; let btu = 1.0;\n}\n";
    assert!(
        has_violations_in_source(source, &rules),
        "// inside a string literal does not create a real comment — must not suppress"
    );
}

#[test]
fn si_guard_ignore_in_block_comment_suppresses_violation() {
    let rules = rules();
    let source = "fn main() {\n    let btu = 1.0; /* si-guard-ignore */\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore in a /* */ block comment should suppress the violation"
    );
}

// ---------------------------------------------------------------------------
// Regression tests — lifetime + si-guard-ignore annotation
// ---------------------------------------------------------------------------
//
// Prior to the lifetime fix, the byte scanner treated every ' as a char
// literal opener and scanned to end-of-line for a closing ', consuming any
// // si-guard-ignore comment that followed a lifetime.  These tests verify
// that lifetimes no longer interfere with annotation recognition.

#[test]
fn lifetime_with_same_line_si_guard_ignore_suppresses_violation() {
    let rules = rules();
    let source = "fn f<'a>() {\n    let btu: &'a f64 = &1.0; // si-guard-ignore: lifetime\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "same-line si-guard-ignore after a lifetime should suppress the violation"
    );
}

#[test]
fn lifetime_with_preceding_line_si_guard_ignore_suppresses_violation() {
    let rules = rules();
    let source = "fn f<'a>() {\n    // si-guard-ignore: preceding annotation\n    let btu: &'a f64 = &1.0;\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "preceding-line si-guard-ignore should suppress a violation on a line that contains a lifetime"
    );
}

#[test]
fn static_lifetime_with_block_comment_si_guard_ignore_suppresses_violation() {
    let rules = rules();
    let source = "fn main() {\n    let btu: &'static f64 = &1.0; /* si-guard-ignore */\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore in a block comment after a 'static lifetime should suppress"
    );
}

#[test]
fn anonymous_lifetime_with_si_guard_ignore_suppresses_violation() {
    let rules = rules();
    let source =
        "fn main() {\n    let btu: &'_ f64 = &1.0; // si-guard-ignore: anonymous lifetime\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore after an anonymous '_ lifetime should suppress"
    );
}

#[test]
fn multiple_lifetimes_with_si_guard_ignore_suppresses_violation() {
    let rules = rules();
    let source = "fn f<'a, 'b>(x: &'a str, y: &'b str) {\n    let btu = 1.0; // si-guard-ignore: multi-lifetime\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore should suppress on a line following multiple lifetimes in a signature"
    );
}

#[test]
fn char_literal_with_si_guard_ignore_still_suppresses() {
    // Verify the lifetime fix does not break char-literal recognition.
    let rules = rules();
    let source =
        "fn main() {\n    let _c = 'x'; let btu = 1.0; // si-guard-ignore: char literal\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore should still suppress on a line containing a char literal"
    );
}

#[test]
fn escaped_quote_char_literal_with_si_guard_ignore_suppresses() {
    let rules = rules();
    let source =
        "fn main() {\n    let _c = '\\''; let btu = 1.0; // si-guard-ignore: escaped quote\n}\n";
    assert!(
        !has_violations_in_source(source, &rules),
        "si-guard-ignore should suppress on a line containing an escaped-quote char literal"
    );
}

#[test]
fn lifetime_without_annotation_flags_violation() {
    // A lifetime without a si-guard-ignore annotation must still be checked.
    let rules = rules();
    let source = "fn main() {\n    let btu: &'a f64 = &1.0;\n}\n";
    assert!(
        has_violations_in_source(source, &rules),
        "btu identifier on a line with a lifetime but no si-guard-ignore must be flagged"
    );
}
