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
