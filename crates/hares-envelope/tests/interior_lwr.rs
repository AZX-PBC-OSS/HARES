//! Interior longwave radiation integration tests.
//!
//! These tests exercise `interior_longwave_net_w` and `interior_longwave_linearised_w`
//! directly via the public API, verifying energy conservation and heat direction.

use hares_envelope::{InteriorSurface, interior_longwave_net_w};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// All surfaces at the same temperature: net LWR flux must sum to zero
/// (energy conservation in radiative equilibrium).
#[test]
fn test_interior_lwr_net_flux_is_zero() {
    let surfaces = vec![
        InteriorSurface { area_m2: 20.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 15.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 25.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 10.0, emissivity: 0.90 },
    ];
    let temps = vec![20.0, 20.0, 20.0, 20.0];

    let fluxes = interior_longwave_net_w(&surfaces, &temps);

    let total: f64 = fluxes.iter().sum();
    assert!(
        total.abs() < 1e-6,
        "net LWR flux sum must be ~0 when all surfaces are at equal temperature, got {total}"
    );

    // Each individual flux should also be ~0 at equal temperatures
    for (i, &q) in fluxes.iter().enumerate() {
        assert!(
            q.abs() < 1e-6,
            "surface {i} flux must be ~0 at equal temperature, got {q}"
        );
    }
}

/// One hot surface (30 C) among three cold surfaces (20 C).
/// The hot surface must lose heat (negative flux) and cold surfaces must gain.
#[test]
fn test_interior_lwr_hot_surface_loses_heat() {
    let surfaces = vec![
        InteriorSurface { area_m2: 20.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 15.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 25.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 10.0, emissivity: 0.90 },
    ];
    let temps = vec![30.0, 20.0, 20.0, 20.0];

    let fluxes = interior_longwave_net_w(&surfaces, &temps);

    assert!(
        fluxes[0] < 0.0,
        "hot surface (30 C) must lose heat (negative flux), got {}",
        fluxes[0]
    );
    assert!(
        fluxes[1] > 0.0,
        "cold surface 1 must gain heat (positive flux), got {}",
        fluxes[1]
    );
    assert!(
        fluxes[2] > 0.0,
        "cold surface 2 must gain heat (positive flux), got {}",
        fluxes[2]
    );
    assert!(
        fluxes[3] > 0.0,
        "cold surface 3 must gain heat (positive flux), got {}",
        fluxes[3]
    );

    // Energy conservation: sum of all fluxes must be ~0
    let total: f64 = fluxes.iter().sum();
    assert!(
        total.abs() < 1e-6,
        "net LWR flux sum must be ~0 (energy conservation), got {total}"
    );
}

/// Two identical surfaces at different temperatures: fluxes must be equal
/// and opposite (energy conservation for a 2-surface enclosure).
#[test]
fn test_interior_lwr_identical_surfaces_symmetric() {
    let surfaces = vec![
        InteriorSurface { area_m2: 20.0, emissivity: 0.90 },
        InteriorSurface { area_m2: 20.0, emissivity: 0.90 },
    ];
    let temps = vec![25.0, 15.0];

    let fluxes = interior_longwave_net_w(&surfaces, &temps);

    assert!(
        (fluxes[0].abs() - fluxes[1].abs()).abs() < 1e-6,
        "|flux_A| must equal |flux_B| for identical surfaces: flux_A={}, flux_B={}",
        fluxes[0],
        fluxes[1]
    );

    // Hot surface loses, cold surface gains
    assert!(
        fluxes[0] < 0.0,
        "surface A (25 C) must lose heat, got {}",
        fluxes[0]
    );
    assert!(
        fluxes[1] > 0.0,
        "surface B (15 C) must gain heat, got {}",
        fluxes[1]
    );

    // Energy conservation
    let total: f64 = fluxes.iter().sum();
    assert!(
        total.abs() < 1e-6,
        "net flux sum must be ~0, got {total}"
    );
}
