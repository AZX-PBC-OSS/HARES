use std::path::Path;

use hares_io::envelope_lut::{EnvelopeLookup, EnvelopeLutError, resolve_boundary_name};
use hares_io::hpxml::building::{BoundaryType, ZoneType};

fn defaults_envelope_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("defaults")
        .join("envelope")
}

// ── Loading ─────────────────────────────────────────────────────────────────

#[test]
fn loads_real_envelope_csvs() {
    let dir = defaults_envelope_dir();
    let lut = EnvelopeLookup::load(&dir).expect("load envelope CSVs");
    // Spot-check: "Exterior Wall" should have at least one boundary type entry.
    let result = lut.lookup("Exterior Wall", None, None, None, Some(10.0));
    assert!(result.is_some(), "Exterior Wall should match something");
    let r = result.unwrap();
    assert!(!r.layers.is_empty(), "should have material layers");
    assert!(r.matched_r_value > 0.0);
}

#[test]
fn missing_directory_returns_error() {
    let err = EnvelopeLookup::load(Path::new("/nonexistent/path")).unwrap_err();
    assert!(matches!(err, EnvelopeLutError::MissingFile(_)));
}

// ── Lookup matching ─────────────────────────────────────────────────────────

#[test]
fn exterior_wall_woodstud_matches_by_construction_type() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    let result = lut
        .lookup("Exterior Wall", Some("WoodStud"), None, None, Some(15.0))
        .expect("should match WoodStud exterior wall");
    assert!(
        result.matched_boundary_type.contains("WoodStud"),
        "matched type should contain WoodStud, got: {}",
        result.matched_boundary_type
    );
    assert!(!result.layers.is_empty());
}

#[test]
fn attic_roof_matches_with_finish_type() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    let result = lut
        .lookup(
            "Attic Roof",
            Some("Pitched"),
            Some("asphalt or fiberglass shingles"),
            None,
            None,
        )
        .expect("should match Attic Roof");
    assert!(!result.layers.is_empty());
}

#[test]
fn attic_floor_r_value_picks_closest() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();

    // R-38 exactly
    let r38 = lut
        .lookup("Attic Floor", None, None, Some("R-38"), None)
        .expect("R-38 attic floor");
    assert!(!r38.layers.is_empty());

    // R-39 should still pick R-38 (closest)
    let r39 = lut
        .lookup("Attic Floor", None, None, None, Some(39.0))
        .expect("R-39 closest match");
    assert!(!r39.layers.is_empty());
}

#[test]
fn unknown_boundary_name_returns_none() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    assert!(
        lut.lookup("Nonexistent Boundary", None, None, None, None)
            .is_none()
    );
}

#[test]
fn window_boundary_not_in_lut() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    // Windows use U-factor path, not LUT
    assert!(lut.lookup("Window", None, None, None, None).is_none());
}

#[test]
fn foundation_wall_has_layers() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    let result = lut
        .lookup("Foundation Wall", None, None, None, Some(5.0))
        .expect("Foundation Wall");
    assert!(!result.layers.is_empty());
    // All layers should have non-negative values
    for layer in &result.layers {
        assert!(layer.resistance_m2_k_w >= 0.0);
        assert!(layer.capacitance_kj_m2_k >= 0.0);
    }
}

#[test]
fn rim_joist_lookup_succeeds() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    let result = lut.lookup("Rim Joist", None, None, None, Some(10.0));
    assert!(result.is_some(), "Rim Joist should be in the LUT");
}

#[test]
fn foundation_floor_lookup_succeeds() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    let result = lut.lookup("Foundation Floor", None, None, None, Some(2.0));
    assert!(result.is_some(), "Foundation Floor should be in the LUT");
}

#[test]
fn high_r_value_treated_as_minimal() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    // OCHRE: R >= 100 → minimal at R500
    let result = lut.lookup("Attic Floor", None, None, None, Some(150.0));
    assert!(result.is_some(), "high R should match Minimal");
    let r = result.unwrap();
    // Minimal attic floor has very high matched R-value (>80 m²·K/W)
    assert!(
        r.matched_r_value > 80.0,
        "should match Minimal variant, got R={}",
        r.matched_r_value
    );
}

// ── Boundary name resolution ────────────────────────────────────────────────

#[test]
fn wall_conditioned_outdoor_resolves_to_exterior_wall() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        ),
        Some("Exterior Wall")
    );
}

#[test]
fn wall_attic_outdoor_resolves_to_attic_wall() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Attic),
            Some(&ZoneType::Outdoor),
        ),
        Some("Attic Wall")
    );
}

#[test]
fn roof_attic_outdoor_resolves_to_attic_roof() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Roof,
            Some(&ZoneType::Attic),
            Some(&ZoneType::Outdoor),
        ),
        Some("Attic Roof")
    );
}

#[test]
fn roof_missing_exterior_defaults_to_outdoor() {
    // HPXML often omits ExteriorAdjacentTo for roofs
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Roof,
            Some(&ZoneType::Attic),
            None, // missing
        ),
        Some("Attic Roof")
    );
}

#[test]
fn floor_conditioned_attic_resolves_to_attic_floor() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Floor,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Attic),
        ),
        Some("Attic Floor")
    );
}

#[test]
fn floor_conditioned_foundation_resolves_to_foundation_ceiling() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Floor,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Foundation),
        ),
        Some("Foundation Ceiling")
    );
}

#[test]
fn slab_foundation_resolves_to_foundation_floor() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Slab,
            Some(&ZoneType::Foundation),
            Some(&ZoneType::Outdoor),
        ),
        Some("Foundation Floor")
    );
}

#[test]
fn slab_missing_exterior_defaults_correctly() {
    assert_eq!(
        resolve_boundary_name(&BoundaryType::Slab, Some(&ZoneType::Foundation), None,),
        Some("Foundation Floor")
    );
}

#[test]
fn rim_joist_resolves() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::RimJoist,
            Some(&ZoneType::Foundation),
            Some(&ZoneType::Outdoor),
        ),
        Some("Rim Joist")
    );
}

#[test]
fn foundation_wall_resolves() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::FoundationWall,
            Some(&ZoneType::Foundation),
            Some(&ZoneType::Outdoor),
        ),
        Some("Foundation Wall")
    );
}

#[test]
fn window_returns_none() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Window,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        ),
        None
    );
}

#[test]
fn garage_wall_resolves() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Garage),
            Some(&ZoneType::Outdoor),
        ),
        Some("Garage Wall")
    );
}

#[test]
fn garage_attached_wall_resolves() {
    assert_eq!(
        resolve_boundary_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Garage),
        ),
        Some("Garage Attached Wall")
    );
}

#[test]
fn both_zones_missing_uses_defaults() {
    // Wall with no zones → defaults to Conditioned interior, Outdoor exterior
    assert_eq!(
        resolve_boundary_name(&BoundaryType::Wall, None, None,),
        Some("Exterior Wall")
    );
}

#[test]
fn high_r_clamps_to_minimal_regardless_of_construction_type() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();
    // R=20.0 exceeds the 17.6 threshold → should clamp to 88.0 and match Minimal,
    // even though construction_type filters to WoodStud rows first.
    let result = lut
        .lookup("Exterior Wall", Some("WoodStud"), None, None, Some(20.0))
        .expect("high R with WoodStud should still match Minimal");
    assert!(
        result.matched_boundary_type.contains("Minimal"),
        "expected Minimal variant, got: {}",
        result.matched_boundary_type
    );
    assert!(
        result.matched_r_value > 80.0,
        "expected high R-value for Minimal, got: {}",
        result.matched_r_value
    );
}

// ── CSV data integrity ──────────────────────────────────────────────────────

/// Regression test: column 9 (0-indexed; column 10 1-indexed) header
/// must read `Specific Heat (J/kg-K)` after the fix for T-0268.
/// The original header `Specific Heat (kJ/kg-K)` was mislabeled — the
/// actual values in that column are in J/kg-K, not kJ/kg-K.
#[test]
fn specific_heat_column_9_header_is_j_per_kg_k() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(&path)
        .expect("open CSV");
    let headers = rdr.headers().expect("read headers");
    let col9 = headers.get(9).expect("column 9 exists");
    assert_eq!(
        col9, "Specific Heat (J/kg-K)",
        "column 9 header must read 'Specific Heat (J/kg-K)' after T-0268 fix"
    );
}

/// Verify that specific heat of GYPSUM BOARD in the envelope materials
/// CSV matches the ASHRAE literature value (~837 J/(kg·K)) within 1%.
/// ASHRAE HoF 2021 Ch.26 Table 4 lists gypsum specific heat at 837 J/(kg·K).
#[test]
fn gypsum_board_specific_heat_matches_ashrae_literature() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(&path)
        .expect("open CSV");
    let headers = rdr.headers().expect("read headers").clone();

    // Column 13 (0-indexed) / 14 (1-indexed) = "Specific Heat (J/kg-K)"
    // — the original J/kg-K column where gypsum's value lives.
    let j_col_idx = headers
        .iter()
        .enumerate()
        .filter(|(_, h)| *h == "Specific Heat (J/kg-K)")
        .map(|(i, _)| i)
        .last()
        .expect("should find column 'Specific Heat (J/kg-K)'");

    for result in rdr.records() {
        let record = result.expect("valid CSV row");
        let material = record.get(5).unwrap_or(""); // Material Name col 6 (1-idx)
        if material.to_uppercase().contains("GYPSUM BOARD") {
            let cp_str = record.get(j_col_idx).unwrap_or("");
            if cp_str.is_empty() {
                continue;
            }
            let cp: f64 = cp_str.parse().expect("parse specific heat");
            let expected = 837.0;
            let tolerance = expected * 0.01; // 1%
            assert!(
                (cp - expected).abs() <= tolerance,
                "GYPSUM BOARD specific heat {} J/kg-K differs from ASHRAE literature value {} by more than 1%",
                cp,
                expected
            );
            return;
        }
    }
    panic!("GYPSUM BOARD row not found in Envelope Materials.csv");
}

/// Verify R = t/k consistency for all STUD AND CAVITY material rows.
///
/// STUD AND CAVITY materials are composites whose t and k represent
/// effective properties from a parallel-path model. R must equal t/k for
/// these rows because the simulation reads R from the CSV and any
/// discordance produces physically inconsistent results.
///
/// Non-STUD-AND-CAVITY rows may have R values rounded or independently
/// sourced and are not checked at 1e-3 tolerance here — those R values
/// may have independent physical justification (e.g. ASHRAE Handbook tables)
/// and changing them would alter simulation outputs without verified physical basis.
#[test]
fn stud_and_cavity_rows_satisfy_r_equals_t_over_k() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let mut rdr = csv::Reader::from_path(&path).expect("open CSV");
    let headers = rdr.headers().expect("read headers").clone();

    let t_idx = headers
        .iter()
        .position(|h| h == "Thickness (m)")
        .expect("Thickness (m) column");
    let k_idx = headers
        .iter()
        .position(|h| h == "Conductivity (W/m-K)")
        .expect("Conductivity (W/m-K) column");
    let r_idx = headers
        .iter()
        .position(|h| h == "Resistance (m^2-K/W)")
        .expect("Resistance (m^2-K/W) column");
    let mat_idx = headers
        .iter()
        .position(|h| h == "Material Name")
        .expect("Material Name column");

    let mut failures: Vec<String> = Vec::new();
    for (i, result) in rdr.records().enumerate() {
        let record = result.expect("valid CSV row");
        let material = record.get(mat_idx).unwrap_or("");
        if !material.contains("STUD AND CAVITY") {
            continue;
        }

        let t_str = record.get(t_idx).unwrap_or("");
        let k_str = record.get(k_idx).unwrap_or("");
        let r_str = record.get(r_idx).unwrap_or("");

        if t_str.is_empty() || k_str.is_empty() || r_str.is_empty() {
            continue;
        }
        let t: f64 = match t_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let k: f64 = match k_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let r: f64 = match r_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        if t <= 0.0 || k <= 0.0 {
            continue;
        }

        let r_computed = t / k;
        let rel_err = (r - r_computed).abs() / r_computed.max(f64::EPSILON);
        if rel_err >= 1e-3 {
            let line = i + 2;
            failures.push(format!(
                "Line {line}: {material}: R={r} does not match t/k={r_computed:.6} (rel_err={rel_err:.4})"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} STUD AND CAVITY rows fail R = t/k within 1e-3:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Regression: the three STUD AND CAVITY rows that had discordant R/k values
/// (T-0269) now satisfy R = t/k.
#[test]
fn minimal_stud_and_cavity_rows_satisfy_r_equals_t_over_k() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let mut rdr = csv::Reader::from_path(&path).expect("open CSV");

    let mut found_ceiling = false;
    let mut found_attic_wall = false;
    let mut found_exterior_wall = false;

    for result in rdr.records() {
        let record = result.expect("valid CSV row");
        let boundary = record.get(0).unwrap_or("");
        let boundary_type = record.get(1).unwrap_or("");
        let material = record.get(5).unwrap_or("");
        let t_str = record.get(6).unwrap_or("");
        let k_str = record.get(7).unwrap_or("");
        let r_str = record.get(10).unwrap_or("");

        if !material.contains("STUD AND CAVITY") || boundary_type != "Minimal" {
            continue;
        }
        if t_str.is_empty() || k_str.is_empty() || r_str.is_empty() {
            continue;
        }

        let t: f64 = t_str.parse().expect("parse thickness");
        let k: f64 = k_str.parse().expect("parse conductivity");
        let r: f64 = r_str.parse().expect("parse resistance");

        if t <= 0.0 || k <= 0.0 {
            continue;
        }

        let r_computed = t / k;
        let rel_err = (r - r_computed).abs() / r_computed.max(f64::EPSILON);
        assert!(
            rel_err < 1e-3,
            "{} / {} / {}: R={} does not match t/k={} (rel_err={:.4})",
            boundary,
            boundary_type,
            material,
            r,
            r_computed,
            rel_err
        );

        match (boundary, material) {
            ("Attic Floor", "CEILING STUD AND CAVITY") => found_ceiling = true,
            ("Attic Wall", "WALL STUD AND CAVITY") => found_attic_wall = true,
            ("Exterior Wall", "WALL STUD AND CAVITY") => found_exterior_wall = true,
            _ => {}
        }
    }

    assert!(
        found_ceiling,
        "did not find Attic Floor Minimal CEILING STUD AND CAVITY"
    );
    assert!(
        found_attic_wall,
        "did not find Attic Wall Minimal WALL STUD AND CAVITY"
    );
    assert!(
        found_exterior_wall,
        "did not find Exterior Wall Minimal WALL STUD AND CAVITY"
    );
}

// ── Envelope Boundaries.csv loading ─────────────────────────────────────────

/// Verify that `Envelope Boundaries.csv` loads successfully and produces
/// known correct entries in the resolved name map.
#[test]
fn envelope_boundaries_csv_loads_successfully() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).expect("load LUT");

    // Spot-check several known entries from the CSV.
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        ),
        Some("Exterior Wall")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Roof,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        ),
        Some("Roof")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Slab,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Ground),
        ),
        Some("Floor")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::FoundationWall,
            Some(&ZoneType::Foundation),
            Some(&ZoneType::Ground),
        ),
        Some("Foundation Wall")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::RimJoist,
            Some(&ZoneType::Foundation),
            Some(&ZoneType::Outdoor),
        ),
        Some("Rim Joist")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Attic),
            Some(&ZoneType::Outdoor),
        ),
        Some("Attic Wall")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Floor,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        ),
        Some("Raised Floor")
    );
}

/// Verify that the CSV-derived boundary names match the hardcoded mapping
/// for every combination where both produce a result.
#[test]
fn csv_names_match_hardcoded_names() {
    use hares_io::hpxml::building::{BoundaryType, ZoneType};

    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).expect("load LUT");

    let all_zones = [
        ZoneType::Conditioned,
        ZoneType::Attic,
        ZoneType::Garage,
        ZoneType::Foundation,
        ZoneType::Outdoor,
        ZoneType::Ground,
        ZoneType::Adjacent,
    ];
    let all_bts = [
        BoundaryType::Wall,
        BoundaryType::Roof,
        BoundaryType::Floor,
        BoundaryType::Window,
        BoundaryType::Door,
        BoundaryType::FoundationWall,
        BoundaryType::RimJoist,
        BoundaryType::Slab,
    ];

    // Window is intentionally excluded: the hardcoded `resolve_boundary_name`
    // returns None for Window boundaries because they use a separate U-factor
    // code path, while the CSV includes "Window" as a valid boundary name.
    // This is a known, intentional divergence — the CSV is correct as a zone
    // mapping; the caller's decision to skip LUT lookup for Windows is a
    // code-path concern, not a mapping concern.
    let mut mismatches = Vec::new();
    for bt in &all_bts {
        if matches!(*bt, BoundaryType::Window | BoundaryType::Skylight) {
            continue;
        }
        for int in &all_zones {
            for ext in &all_zones {
                let csv_name = lut.resolve_name(bt, Some(int), Some(ext));
                let hc_name =
                    hares_io::envelope_lut::resolve_boundary_name(bt, Some(int), Some(ext));
                match (csv_name, hc_name) {
                    (Some(c), Some(h)) if c != h => {
                        mismatches.push(format!(
                            "({bt:?}, {int:?}, {ext:?}): CSV={c:?}, hardcoded={h:?}"
                        ));
                    }
                    (Some(_), None) | (None, Some(_)) => {
                        mismatches.push(format!(
                            "({bt:?}, {int:?}, {ext:?}): one has result, other doesn't"
                        ));
                    }
                    _ => {}
                }
            }
        }
    }

    if !mismatches.is_empty() {
        // Sort for deterministic output.
        mismatches.sort();
        panic!(
            "{} mismatches between CSV-derived and hardcoded boundary names:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        );
    }
}

/// Regression: editing a zone label in `Envelope Boundaries.csv` changes
/// the resolved boundary name.
#[test]
fn csv_edit_changes_resolved_boundary_name() {
    let dir = tempfile::tempdir().expect("create temp dir");

    // Copy the two required CSVs
    let env_dir = defaults_envelope_dir();
    for file_name in &["Envelope Boundary Types.csv", "Envelope Materials.csv"] {
        std::fs::copy(env_dir.join(file_name), dir.path().join(file_name))
            .expect("copy required CSV");
    }

    // Write a minimal Boundaries.csv with a known entry.
    let boundaries_content = "\
Boundary Name,Boundary Label,Exterior Zone Label,Interior Zone Label
Exterior Wall,EW,EXT,LIV
Garage Wall,GW,EXT,GAR
";
    let boundaries_path = dir.path().join("Envelope Boundaries.csv");
    std::fs::write(&boundaries_path, boundaries_content).expect("write CSV");

    let lut = EnvelopeLookup::load(dir.path()).expect("load LUT");

    // Garage Wall should resolve from CSV
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Garage),
            Some(&ZoneType::Outdoor),
        ),
        Some("Garage Wall")
    );

    // Now edit the CSV — change the boundary name for EXT/GAR from
    // "Garage Wall" to "Attic Wall" (still recognized by the classifier).
    let boundaries_content_v2 = "\
Boundary Name,Boundary Label,Exterior Zone Label,Interior Zone Label
Exterior Wall,EW,EXT,LIV
Attic Wall,AW,EXT,GAR
";
    std::fs::write(&boundaries_path, boundaries_content_v2).expect("rewrite CSV");

    let lut2 = EnvelopeLookup::load(dir.path()).expect("reload LUT");
    assert_eq!(
        lut2.resolve_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Garage),
            Some(&ZoneType::Outdoor),
        ),
        Some("Attic Wall"),
        "CSV edit should change the resolved name"
    );
}

/// Verify that `resolve_name` returns "Window" for Window boundaries
/// (the CSV is the source of truth; callers that skip LUT lookup for
/// Windows do so at the code-path level, not the mapping level).
#[test]
fn resolve_name_returns_window_from_csv() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).expect("load LUT");
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Window,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Outdoor),
        ),
        Some("Window")
    );
}

/// Verify that `resolve_name` returns furniture names for same-zone combinations.
#[test]
fn resolve_name_returns_furniture_for_same_zone() {
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).expect("load LUT");
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Wall,
            Some(&ZoneType::Conditioned),
            Some(&ZoneType::Conditioned),
        ),
        Some("Indoor Furniture")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Floor,
            Some(&ZoneType::Foundation),
            Some(&ZoneType::Foundation),
        ),
        Some("Foundation Furniture")
    );
    assert_eq!(
        lut.resolve_name(
            &BoundaryType::Roof,
            Some(&ZoneType::Garage),
            Some(&ZoneType::Garage),
        ),
        Some("Garage Furniture")
    );
}

/// Verify that `Envelope Boundary Types.csv` contains Window assembly rows
/// with physically correct R-values (R = 1/U for typical window U-factors).
/// The rows document reference window thermal performance by vintage; actual
/// window modelling uses the Window struct U-factor path (EnergyPlus Simple
/// Window Model Step 1), not the LUT. This test ensures the data completeness
/// gap is closed and the values are physically plausible.
///
/// ASHRAE HoF 2021 Ch.15 Table 4: typical center-of-glass U-factors.
#[test]
fn window_boundary_type_rows_in_csv() {
    let path = defaults_envelope_dir().join("Envelope Boundary Types.csv");
    let mut rdr = csv::Reader::from_path(&path).expect("open CSV");
    let headers = rdr.headers().expect("read headers").clone();

    let name_idx = headers
        .iter()
        .position(|h| h == "Boundary Name")
        .expect("Boundary Name column");
    let r_idx = headers
        .iter()
        .position(|h| h == "Assembly R Value")
        .expect("Assembly R Value column");

    let mut window_rows: Vec<(String, f64)> = Vec::new();
    for result in rdr.records() {
        let record = result.expect("valid CSV row");
        let name = record.get(name_idx).unwrap_or("");
        if name != "Window" {
            continue;
        }
        // Column 1 is the boundary type (window vintage label).
        let bt = record.get(1).unwrap_or("");
        let r_str = record.get(r_idx).unwrap_or("");
        let r: f64 = r_str.parse().expect("parse Assembly R Value");
        window_rows.push((bt.to_string(), r));
    }

    assert!(
        !window_rows.is_empty(),
        "Envelope Boundary Types.csv must contain Window rows"
    );

    // Verify the four expected vintage rows with physically correct R-values.
    // R = 1/U for each window type; U-factors from ASHRAE HoF 2021 Ch.15.
    let expected: &[(&str, f64)] = &[
        // U ≈ 5.68 W/m²K → R = 1/5.678 = 0.1761 m²K/W
        ("Single-Pane Aluminum", 0.1761),
        // U ≈ 2.89 W/m²K → R = 1/2.890 = 0.3460 m²K/W
        ("Double-Pane Clear", 0.3460),
        // U ≈ 1.77 W/m²K → R = 1/1.770 = 0.5650 m²K/W
        ("Double-Pane Low-E", 0.5650),
        // U ≈ 0.98 W/m²K → R = 1/0.980 = 1.0204 m²K/W
        ("Triple-Pane Low-E", 1.0204),
    ];

    for &(exp_name, exp_r) in expected {
        let found = window_rows.iter().find(|(bt, _)| bt == exp_name);
        assert!(
            found.is_some(),
            "Window row '{}' not found in Boundary Types CSV",
            exp_name
        );
        let (_, actual_r) = found.unwrap();
        let rel_err = (actual_r - exp_r).abs() / exp_r.max(f64::EPSILON);
        assert!(
            rel_err < 1e-3,
            "Window '{}': Assembly R Value {:.6} differs from expected {:.6} (rel_err={:.4})",
            exp_name,
            actual_r,
            exp_r,
            rel_err,
        );
    }
}

// ── Stud cavity R-value monotonicity ─────────────────────────────────────

/// Split a variant label into (prefix, insulation_rank).
fn split_r_variant(variant: &str) -> Option<(&str, u32)> {
    if variant.eq_ignore_ascii_case("Uninsulated") {
        return Some(("", 0));
    }
    if let Some(r_pos) = variant.find("R-") {
        let after_r = &variant[r_pos + 2..];
        let digits_end = after_r
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(after_r.len());
        let digits_str = &after_r[..digits_end];
        if let Ok(rank) = digits_str.parse::<f64>() {
            let int_rank = rank as u32;
            let prefix = &variant[..r_pos];
            return Some((prefix, int_rank));
        }
    }
    None
}

/// Verify that for Foundation Ceiling, Garage Interior Ceiling, and
/// Raised Floor groups (the boundary names fixed in T-0272), the stud
/// cavity R-value increases monotonically with insulation level.
#[test]
fn stud_cavity_r_values_are_monotonic() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let mut rdr = csv::Reader::from_path(&path).expect("open CSV");
    let headers = rdr.headers().expect("read headers").clone();

    let name_idx = headers
        .iter()
        .position(|h| h == "Boundary Name")
        .expect("Boundary Name column");
    let bt_idx = headers
        .iter()
        .position(|h| h == "Boundary Type")
        .expect("Boundary Type column");
    let mat_idx = headers
        .iter()
        .position(|h| h == "Material Name")
        .expect("Material Name column");
    let r_idx = headers
        .iter()
        .position(|h| h == "Resistance (m^2-K/W)")
        .expect("Resistance column");

    let target_names = [
        "Foundation Ceiling",
        "Garage Interior Ceiling",
        "Raised Floor",
    ];

    use std::collections::BTreeMap;
    // (boundary_name, variant_prefix) → rank → resistance
    let mut groups: BTreeMap<(String, String), BTreeMap<u32, f64>> = BTreeMap::new();

    for result in rdr.records() {
        let record = result.expect("valid CSV row");
        let boundary_name = record.get(name_idx).unwrap_or("");
        if !target_names.contains(&boundary_name) {
            continue;
        }
        let material = record.get(mat_idx).unwrap_or("");
        if !material.contains("STUD AND CAVITY") {
            continue;
        }
        let boundary_type = record.get(bt_idx).unwrap_or("");
        let r_str = record.get(r_idx).unwrap_or("");

        if boundary_type.is_empty() || r_str.is_empty() {
            continue;
        }
        let r: f64 = match r_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (prefix, rank) = match split_r_variant(boundary_type) {
            Some(v) => v,
            None => continue,
        };
        groups
            .entry((boundary_name.to_string(), prefix.to_string()))
            .or_default()
            .insert(rank, r);
    }

    assert!(
        !groups.is_empty(),
        "should find at least one group with STUD AND CAVITY rows in the fixed boundary names"
    );

    let mut failures = Vec::new();
    for ((boundary_name, prefix), variants) in &groups {
        if variants.len() < 2 {
            continue;
        }
        let ordered: Vec<(u32, f64)> = variants.iter().map(|(rank, r)| (*rank, *r)).collect();
        for window in ordered.windows(2) {
            let (prev_rank, prev_r) = window[0];
            let (next_rank, next_r) = window[1];
            if next_r < prev_r {
                failures.push(format!(
                    "{boundary_name} / \"{prefix}\": rank {prev_rank} (R={prev_r:.4}) → rank {next_rank} (R={next_r:.4})"
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} boundary name/prefix groups have non-monotonic stud cavity R-values:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Regression: Foundation Ceiling, Garage Interior Ceiling, and Raised Floor
/// R-38 stud cavity layers have R-values >= their R-19 counterparts.
#[test]
fn r38_stud_cavity_r_gte_r19() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let headers = {
        let mut rdr = csv::Reader::from_path(&path).expect("open CSV");
        rdr.headers().expect("read headers").clone()
    };

    let name_idx = headers
        .iter()
        .position(|h| h == "Boundary Name")
        .expect("Boundary Name column");
    let bt_idx = headers
        .iter()
        .position(|h| h == "Boundary Type")
        .expect("Boundary Type column");
    let mat_idx = headers
        .iter()
        .position(|h| h == "Material Name")
        .expect("Material Name column");
    let r_idx = headers
        .iter()
        .position(|h| h == "Resistance (m^2-K/W)")
        .expect("Resistance column");

    let target_names = [
        "Foundation Ceiling",
        "Garage Interior Ceiling",
        "Raised Floor",
    ];

    for &target_name in &target_names {
        let mut r38_r: Option<f64> = None;
        let mut r19_r: Option<f64> = None;

        let mut rdr2 = csv::Reader::from_path(&path).expect("reopen CSV");
        for result in rdr2.records() {
            let record = result.expect("valid CSV row");
            let boundary_name = record.get(name_idx).unwrap_or("");
            if boundary_name != target_name {
                continue;
            }
            let material = record.get(mat_idx).unwrap_or("");
            if !material.contains("STUD AND CAVITY") {
                continue;
            }
            let boundary_type = record.get(bt_idx).unwrap_or("");
            let r_str = record.get(r_idx).unwrap_or("");
            if r_str.is_empty() {
                continue;
            }
            let r: f64 = r_str.parse().expect("parse resistance");
            if let Some((_, rank)) = split_r_variant(boundary_type) {
                if rank == 38 {
                    r38_r = Some(r);
                }
                if rank == 19 {
                    r19_r = Some(r);
                }
            }
        }

        let r38 = r38_r.expect(&format!("{target_name} should have an R-38 variant"));
        let r19 = r19_r.expect(&format!("{target_name} should have an R-19 variant"));
        assert!(
            r38 >= r19,
            "{target_name}: R-38 stud cavity R={r38:.4} should be >= R-19 stud cavity R={r19:.4}"
        );
    }
}

/// Verify that no INSULATED stud cavity layer in the fixed boundary names
/// (Foundation Ceiling, Garage Interior Ceiling, Raised Floor) has implausibly
/// high conductivity.
///
/// ASHRAE HoF 2021 Ch.26: still air k ≈ 0.026 W/m·K. An insulated stud
/// cavity typically has k_eff ≈ 0.04–0.10 W/m·K. Values above 0.5 W/m·K
/// for an insulated cavity are physically implausible and indicate the
/// back-calculation forced the layer to act as a thermal short.
#[test]
fn no_insulated_stud_cavity_conductivity_exceeds_plausible_maximum() {
    let path = defaults_envelope_dir().join("Envelope Materials.csv");
    let mut rdr = csv::Reader::from_path(&path).expect("open CSV");
    let headers = rdr.headers().expect("read headers").clone();

    let mat_idx = headers
        .iter()
        .position(|h| h == "Material Name")
        .expect("Material Name column");
    let k_idx = headers
        .iter()
        .position(|h| h == "Conductivity (W/m-K)")
        .expect("Conductivity column");
    let name_idx = headers
        .iter()
        .position(|h| h == "Boundary Name")
        .expect("Boundary Name column");
    let bt_idx = headers
        .iter()
        .position(|h| h == "Boundary Type")
        .expect("Boundary Type column");

    let target_names = [
        "Foundation Ceiling",
        "Garage Interior Ceiling",
        "Raised Floor",
    ];

    let max_plausible_k: f64 = 0.5;
    let mut violations: Vec<String> = Vec::new();

    for result in rdr.records() {
        let record = result.expect("valid CSV row");
        let boundary_name = record.get(name_idx).unwrap_or("");
        if !target_names.contains(&boundary_name) {
            continue;
        }
        let material = record.get(mat_idx).unwrap_or("");
        if !material.contains("STUD AND CAVITY") {
            continue;
        }
        let boundary_type = record.get(bt_idx).unwrap_or("");
        // Skip uninsulated cavities — they naturally have high k.
        if let Some((_, rank)) = split_r_variant(boundary_type) {
            if rank == 0 {
                continue;
            }
        }
        let k_str = record.get(k_idx).unwrap_or("");
        if k_str.is_empty() {
            continue;
        }
        let k: f64 = k_str.parse().expect("parse conductivity");
        if k > max_plausible_k {
            violations.push(format!(
                "{boundary_name} / {boundary_type} / {material}: k={k:.5} W/m·K > {max_plausible_k}"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "{} insulated STUD AND CAVITY layers in fixed boundary names exceed plausible maximum conductivity {} W/m·K:\n{}",
        violations.len(),
        max_plausible_k,
        violations.join("\n")
    );
}

// ── T-0274: Foundation Wall (Walkout) zone mapping ───────────────────────────

#[test]
fn foundation_wall_walkout_resolves_correct_boundary_name() {
    // A Wall between Foundation and Outdoor represents a walkout basement wall
    // exposed to ambient air. It must return "Foundation Wall (Walkout)", not
    // "Exterior Wall" (which has wood-framed thermal properties).
    let name = resolve_boundary_name(
        &BoundaryType::Wall,
        Some(&ZoneType::Foundation),
        Some(&ZoneType::Outdoor),
    );
    assert_eq!(name, Some("Foundation Wall (Walkout)"));
}

#[test]
fn foundation_wall_walkout_lookup_returns_concrete_assemblies() {
    // Regression: the walkout foundation wall lookup must return concrete-based
    // assemblies with high thermal mass (capacitance > 200 kJ/m²·K), not
    // wood-framed assemblies that are inappropriate for foundation construction.
    let lut = EnvelopeLookup::load(&defaults_envelope_dir()).unwrap();

    // Verify the walkout boundary name resolves through the lookup.
    let result = lut
        .lookup("Foundation Wall (Walkout)", None, None, None, None)
        .expect("Foundation Wall (Walkout) should have at least one assembly");

    assert!(!result.layers.is_empty(), "should have material layers");

    // Total capacitance across all layers (thermal mass).
    let total_capacitance: f64 = result.layers.iter().map(|l| l.capacitance_kj_m2_k).sum();
    assert!(
        total_capacitance > 200.0,
        "walkout foundation wall assembly thermal mass ({:.1} kJ/m²·K) should exceed 200 kJ/m²·K for concrete construction",
        total_capacitance
    );

    // Assembly R-value should be positive (not a zero-thickness placeholder).
    assert!(result.matched_r_value > 0.0);

    // Spot-check that construction is concrete-based, not wood-framed.
    assert!(
        !result
            .matched_boundary_type
            .to_lowercase()
            .contains("woodstud"),
        "walkout foundation wall matched a wood-framed assembly: {}",
        result.matched_boundary_type
    );
}

#[test]
fn csv_boundaries_no_wildcard_fallthrough_for_defined_entries() {
    // Iterate every non-adjacent, non-furniture row in Envelope Boundaries.csv
    // and verify that resolve_boundary_name() returns the expected boundary name.
    // Any mismatch means the CSV and hardcoded mapping have diverged, which would
    // cause the wildcard fallback to be hit for a defined entry.
    use hares_io::envelope_lut::resolve_boundary_name;
    use std::collections::HashSet;

    let boundaries_path = defaults_envelope_dir().join("Envelope Boundaries.csv");
    let mut rdr = csv::Reader::from_path(&boundaries_path).expect("open Envelope Boundaries.csv");

    // Re-import zone_label_to_type and boundary_type_from_boundary_name from the
    // crate-internal helpers. Since they are not pub, we inline minimal copies
    // for the test.
    fn zone_label_to_type(label: &str) -> Option<ZoneType> {
        match label {
            "LIV" => Some(ZoneType::Conditioned),
            "EXT" => Some(ZoneType::Outdoor),
            "GND" => Some(ZoneType::Ground),
            "FND" => Some(ZoneType::Foundation),
            "ATC" => Some(ZoneType::Attic),
            "GAR" => Some(ZoneType::Garage),
            _ => None,
        }
    }
    fn boundary_type_from_name(name: &str) -> Option<BoundaryType> {
        match name {
            "Exterior Wall"
            | "Interior Wall"
            | "Attic Wall"
            | "Garage Wall"
            | "Garage Attached Wall"
            | "Adjacent Wall"
            | "Adjacent Attic Wall"
            | "Adjacent Garage Wall"
            | "Foundation Wall (Walkout)" => Some(BoundaryType::Wall),
            "Roof" | "Attic Roof" | "Garage Roof" | "Adjacent Ceiling" => Some(BoundaryType::Roof),
            "Raised Floor"
            | "Attic Floor"
            | "Foundation Ceiling"
            | "Garage Interior Ceiling"
            | "Garage Ceiling"
            | "Adjacent Floor" => Some(BoundaryType::Floor),
            "Floor" | "Foundation Floor" | "Garage Floor" => Some(BoundaryType::Slab),
            "Window" => Some(BoundaryType::Window),
            "Skylight" => Some(BoundaryType::Skylight),
            "Door" | "Garage Door" => Some(BoundaryType::Door),
            "Foundation Wall" | "Adjacent Foundation Wall" => Some(BoundaryType::FoundationWall),
            "Rim Joist" | "Adjacent Rim Joist" => Some(BoundaryType::RimJoist),
            // Furniture and unrecognized names are intentionally excluded from the mapping.
            _ => None,
        }
    }

    // Boundary names that do not participate in the hardcoded mapping because
    // they are same-zone (furniture) or adjacent-only entries.
    let excluded: HashSet<&str> = [
        "Indoor Furniture",
        "Foundation Furniture",
        "Attic Furniture",
        "Garage Furniture",
        "Interior Wall", // same-zone (LIV/LIV)
    ]
    .into_iter()
    .collect();

    // Adjacent entries are excluded — their resolution is handled by the
    // hardcoded adjacent path, not the CSV-derived map.
    let skip_adjacent = |name: &str| -> bool { name.starts_with("Adjacent") };

    // Skylight and Window return None — they bypass the LUT.
    let skip_none = |name: &str| -> bool { matches!(name, "Window" | "Skylight") };

    let mut checked = 0usize;
    for result in rdr.records() {
        let record = result.expect("read CSV record");
        let boundary_name = record.get(0).unwrap_or("");
        if excluded.contains(boundary_name)
            || skip_adjacent(boundary_name)
            || skip_none(boundary_name)
        {
            continue;
        }
        let ext_label = record.get(2).unwrap_or("");
        let int_label = record.get(3).unwrap_or("");

        let ext = match zone_label_to_type(ext_label) {
            Some(z) => z,
            None => continue,
        };
        let int = match zone_label_to_type(int_label) {
            Some(z) => z,
            None => continue,
        };
        let bt = match boundary_type_from_name(boundary_name) {
            Some(b) => b,
            None => continue,
        };

        let resolved = resolve_boundary_name(&bt, Some(&int), Some(&ext));
        assert_eq!(
            resolved,
            Some(boundary_name),
            "CSV row '{}' ({:?} / {:?}): resolve_boundary_name returned {:?}, expected Some(\"{}\")",
            boundary_name,
            bt,
            (int, ext),
            resolved,
            boundary_name
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "no CSV rows were checked; test may be misconfigured"
    );
}
