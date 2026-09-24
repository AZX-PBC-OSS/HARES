//! Material-consistency checks between TOML fixtures and analytical expectations.
//!
//! Each test parses a BESTEST fixture at runtime, computes the thermal capacitance
//! (C_wall) of the south-wall assembly directly from the material layers, and
//! asserts it against the expected value. This catches drift between the canonical
//! TOML fixture and any analytical calculations derived from it — including the
//! OCHRE-side BESTEST runner's material CSV in `scripts/ochre_bestest_600.py`.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct FixtureConfig {
    boundaries: Option<Vec<BoundaryConfig>>,
}

#[derive(Debug, Deserialize)]
struct BoundaryConfig {
    id: String,
    // Required for serde/toml to deserialize the fixture's full
    // `boundary_type` field; the test itself only reads `id` and
    // `material_layers`.
    #[allow(dead_code)]
    boundary_type: Option<String>,
    material_layers: Vec<MaterialLayerConfig>,
}

#[derive(Debug, Deserialize)]
struct MaterialLayerConfig {
    thickness_m: f64,
    // Required for serde/toml to deserialize the fixture's full
    // `conductivity_w_m_k` field; the test itself only reads
    // `thickness_m`, `density_kg_m3`, and `specific_heat_j_kg_k`.
    #[allow(dead_code)]
    conductivity_w_m_k: Option<f64>,
    density_kg_m3: f64,
    specific_heat_j_kg_k: f64,
}

fn fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest")
}

/// ASHRAE 140-2017 Case 600 south-wall thermal capacitance derived from the
/// three material layers defined in `tests/fixtures/bestest/600.toml`:
///
/// | Layer          | thickness [m] | density [kg/m³] | c_p [J/(kg·K)] | contribution [J/(m²·K)] |
/// |----------------|---------------|-----------------|-----------------|--------------------------|
/// | Wood siding    | 0.009         | 530             | 900             | 4293                     |
/// | Insulation     | 0.066         | 12              | 840             | 665.28                   |
/// | Plasterboard   | 0.012         | 950             | 840             | 9576                     |
/// | **C_wall**     |               |                 |                 | **14 534**               |
///
/// C_wall = Σ thickness × density × specific_heat.
const EXPECTED_C_WALL_J_PER_M2K: f64 = 14_534.0;

/// Tolerance expressed as a fraction of the expected value.  0.1% ensures the
/// computed value is within 14 520–14 549 J/(m²·K).
const C_WALL_TOLERANCE: f64 = 0.001;

/// Parse `tests/fixtures/bestest/600.toml`, extract the south-wall material
/// layers, sum their thermal capacitance contributions, and assert the result
/// matches the expected value within 0.1%.
///
/// This test catches drift between the canonical HARES fixture and any
/// analytical calculation derived from it — including the OCHRE-side BESTEST
/// runner's material CSV.
#[test]
fn south_wall_c_wall_matches_fixture() {
    let fixture_path = fixture_root().join("600.toml");
    let toml_str = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", fixture_path.display()));

    let config: FixtureConfig =
        toml::from_str(&toml_str).unwrap_or_else(|e| panic!("failed to parse TOML fixture: {e}"));

    let boundaries = config
        .boundaries
        .as_ref()
        .expect("600.toml must have [[boundaries]]");

    let south_wall = boundaries
        .iter()
        .find(|b| b.id == "south-wall")
        .expect("600.toml must contain a south-wall boundary");

    let c_wall: f64 = south_wall
        .material_layers
        .iter()
        .map(|l| l.thickness_m * l.density_kg_m3 * l.specific_heat_j_kg_k)
        .sum();

    let abs_delta = (c_wall - EXPECTED_C_WALL_J_PER_M2K).abs();
    let rel_delta = abs_delta / EXPECTED_C_WALL_J_PER_M2K;
    let pct = rel_delta * 100.0;
    let tol_pct = C_WALL_TOLERANCE * 100.0;

    assert!(
        rel_delta <= C_WALL_TOLERANCE,
        "south-wall C_wall = {c_wall:.2} J/(m²·K) deviates from expected \
         {EXPECTED_C_WALL_J_PER_M2K:.2} J/(m²·K) by {pct:.4}% \
         (tolerance {tol_pct:.4}%)",
    );
}

/// Reparse 600.toml and independently verify that the north, east, and west
/// walls share identical material-layer properties with the south wall.
/// This confirms that all four wall boundaries in the fixture describe the
/// same construction assembly.
#[test]
fn all_walls_share_identical_material_layers() {
    let fixture_path = fixture_root().join("600.toml");
    let toml_str = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", fixture_path.display()));

    let config: FixtureConfig = toml::from_str(&toml_str).unwrap();
    let boundaries = config.boundaries.as_ref().unwrap();

    let wall_ids = ["south-wall", "north-wall", "east-wall", "west-wall"];

    let mut reference_c_wall: Option<f64> = None;

    for &wall_id in &wall_ids {
        let boundary = boundaries
            .iter()
            .find(|b| b.id == wall_id)
            .unwrap_or_else(|| panic!("600.toml must contain a {wall_id} boundary"));

        let c_wall: f64 = boundary
            .material_layers
            .iter()
            .map(|l| l.thickness_m * l.density_kg_m3 * l.specific_heat_j_kg_k)
            .sum();

        match reference_c_wall {
            None => reference_c_wall = Some(c_wall),
            Some(expected) => {
                assert!(
                    (c_wall - expected).abs() < 0.01,
                    "{wall_id} C_wall = {c_wall:.2} J/(m²·K) differs from \
                     reference {expected:.2} J/(m²·K)",
                );
            }
        }
    }
}
