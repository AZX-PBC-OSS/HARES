//! Structural envelope tests: verify the *structure* of the RC model built
//! from BEopt_example.xml. UA parity is checked against the independently
//! derived ASHRAE/EnergyPlus reference at
//! `tests/fixtures/parity/ashrae_rc_reference.json` (see its `_provenance`
//! block for citations). OCHRE values are quoted alongside for ballpark
//! comparison only -- OCHRE is NOT a correctness oracle for HARES.
//!
//! Unlike `envelope_oracle.rs` (which runs a dynamic simulation and compares
//! thermal output), this test validates the *static* construction: zone count,
//! boundary areas, zone-to-zone connections, RC topology, and capacitances.

mod tests {
    use std::path::PathBuf;

    use hares_core::{
        building_to_boundary_inputs, building_to_zone_inputs, mass_multiplier_for_zone,
    };
    use hares_envelope::{ExteriorTarget, InteriorLwrMethod, RCPath, assemble_building_rc, derive_zone_capacitances};
    use hares_io::envelope_lut::resolve_boundary_name;
    use hares_io::hpxml::{BoundaryType, ZoneType};
    use hares_io::{DefaultsStore, parse_hpxml};
    use hares_physics::air_properties::standard_pressure_pa;

    /// Compute site pressure from building elevation, falling back to sea level.
    fn site_pressure_for_building(building: &hares_io::Building) -> f64 {
        standard_pressure_pa(building.site.elevation_m.unwrap_or(0.0))
    }

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn beopt_xml_path() -> PathBuf {
        project_root().join("data/examples/BEopt_example.xml")
    }

    // ── OCHRE reference constants (from extract_structure.py) ─────────────

    // Zone volumes [m³]
    const OCHRE_INDOOR_VOLUME_M3: f64 = 271.841727;
    // OCHRE attic volume: 0.5 * floor_area * sqrt(gable_area * tan(pitch))
    // Ref: ochre/utils/hpxml.py parse_hpxml_zones(), gable_area=13.42 m², pitch=6/12
    const OCHRE_ATTIC_VOLUME_M3: f64 = 144.415918;

    // Zone capacitances [J/K]: C = rho * cp * V * TCM
    // OCHRE uses rho=1.2041 kg/m³; HARES uses 1.2 kg/m³ → ~0.3% difference
    const OCHRE_INDOOR_CAPACITANCE_JK: f64 = 1.2041 * 1006.0 * OCHRE_INDOOR_VOLUME_M3 * 7.0;
    const OCHRE_ATTIC_CAPACITANCE_JK: f64 = 1.2041 * 1006.0 * OCHRE_ATTIC_VOLUME_M3 * 7.0;

    // Boundary surface areas from HPXML [m²]
    const EXTERIOR_WALL_TOTAL_M2: f64 = 104.051405;
    const ATTIC_WALL_TOTAL_M2: f64 = 26.849;
    const ATTIC_ROOF_TOTAL_M2: f64 = 124.6425;
    const ATTIC_FLOOR_M2: f64 = 111.483648;
    const SLAB_M2: f64 = 111.483648;
    const DOOR_M2: f64 = 1.8581;
    const WINDOW_TOTAL_M2: f64 = 15.607711;
    const WINDOW_COUNT: usize = 6;
    const WINDOW_U_FACTOR_SI: f64 = 2.100957; // 0.37 Btu/(h·ft²·°F) → W/(m²·K)
    const WINDOW_SHGC: f64 = 0.3;

    // Solar absorptance from HPXML <SolarAbsorptance> elements
    const WALL_ABSORPTANCE: f64 = 0.75;
    const ROOF_ABSORPTANCE: f64 = 0.85;

    // OCHRE effective UA values [W/K] -- computed with TARP/DOE-2 film R (wind=2 m/s,
    // T_ambient=10°C, T_ground=10°C) and same-zone halving. Like-for-like with HARES.
    #[allow(dead_code)]
    const OCHRE_EXTERIOR_WALL_UA: f64 = 34.34;
    #[allow(dead_code)]
    const OCHRE_ATTIC_WALL_UA: f64 = 27.14;
    #[allow(dead_code)]
    const OCHRE_ATTIC_FLOOR_UA: f64 = 17.37;
    #[allow(dead_code)]
    const OCHRE_FLOOR_SLAB_UA: f64 = 72.81;
    #[allow(dead_code)]
    const OCHRE_ATTIC_ROOF_UA: f64 = 139.31;
    #[allow(dead_code)]
    const OCHRE_WINDOW_UA: f64 = 5.77;
    #[allow(dead_code)]
    const OCHRE_DOOR_UA: f64 = 1.59;
    #[allow(dead_code)]
    const OCHRE_INTERIOR_WALL_UA: f64 = 215.15;
    #[allow(dead_code)]
    const OCHRE_INDOOR_FURNITURE_UA: f64 = 45.25;
    #[allow(dead_code)]
    const OCHRE_TOTAL_UA: f64 = 558.74;

    // ── Helpers ───────────────────────────────────────────────────────────

    /// Assert `actual` is within `pct`% of `expected`.
    /// Requires `expected.abs() > 1e-9`; panics otherwise to avoid meaningless comparisons.
    fn assert_within_pct(actual: f64, expected: f64, pct: f64, label: &str) {
        assert!(
            expected.abs() > 1e-9,
            "{label}: expected value {expected} is too close to zero for relative comparison"
        );
        let diff = ((actual - expected) / expected * 100.0).abs();
        assert!(
            diff <= pct,
            "{label}: actual={actual:.4} expected={expected:.4} diff={diff:.2}% (tolerance={pct}%)"
        );
    }

    fn zone_index(building: &hares_io::Building, zone_type: ZoneType) -> usize {
        building
            .zones
            .iter()
            .position(|z| z.zone_type == zone_type)
            .unwrap_or_else(|| panic!("zone type {zone_type:?} not found in building"))
    }

    // ── Test 1: HPXML parse correctness ──────────────────────────────────

    #[test]
    fn beopt_building_structure() {
        let building = parse_hpxml(&beopt_xml_path()).expect("parse BEopt HPXML");

        // Zone count: Conditioned + Attic + Outdoor
        assert!(
            building.zones.len() >= 2,
            "expected at least 2 zones, got {}",
            building.zones.len()
        );
        let conditioned = building
            .zones
            .iter()
            .find(|z| z.zone_type == ZoneType::Conditioned)
            .expect("conditioned zone must exist");
        let attic = building
            .zones
            .iter()
            .find(|z| z.zone_type == ZoneType::Attic)
            .expect("attic zone must exist");

        // Zone volumes
        let cond_vol = conditioned
            .volume_m3
            .expect("conditioned volume must be set");
        assert_within_pct(cond_vol, OCHRE_INDOOR_VOLUME_M3, 1.0, "conditioned volume");

        // Attic volume: computed from gable wall area + roof pitch (triangular prism).
        let attic_vol = attic
            .volume_m3
            .expect("attic volume must be computed from gable geometry");
        assert_within_pct(attic_vol, OCHRE_ATTIC_VOLUME_M3, 2.0, "attic volume");

        // Floor area
        let cond_area = conditioned.floor_area_m2.expect("conditioned floor area");
        assert_within_pct(cond_area, ATTIC_FLOOR_M2, 1.0, "conditioned floor area");

        // Boundary counts by type
        let walls: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Wall)
            .collect();
        let roofs: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Roof)
            .collect();
        let floors: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Floor)
            .collect();
        let slabs: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Slab)
            .collect();
        let doors: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Door)
            .collect();
        let windows: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Window)
            .collect();

        // HPXML-parsed walls: 4 exterior + 2 attic gable = 6.
        // Auto-generated: interior_wall, conditioned_furniture, garage_furniture, foundation_furniture.
        let hpxml_walls: Vec<_> = walls
            .iter()
            .filter(|b| !b.id.contains("furniture") && b.id != "interior_wall")
            .collect();
        assert_eq!(
            hpxml_walls.len(),
            6,
            "expected 6 HPXML walls (4 exterior + 2 attic gable)"
        );
        assert_eq!(roofs.len(), 2, "expected 2 roof surfaces");
        assert_eq!(floors.len(), 1, "expected 1 floor (attic floor / ceiling)");
        assert_eq!(slabs.len(), 1, "expected 1 slab");
        assert_eq!(doors.len(), 1, "expected 1 door");
        assert_eq!(
            windows.len(),
            WINDOW_COUNT,
            "expected {WINDOW_COUNT} windows"
        );

        // HPXML wall areas (net of window/door subtraction).
        let wall_area: f64 = hpxml_walls.iter().map(|b| b.area_m2).sum();
        let expected_net_wall_area =
            EXTERIOR_WALL_TOTAL_M2 + ATTIC_WALL_TOTAL_M2 - WINDOW_TOTAL_M2 - DOOR_M2;
        assert_within_pct(
            wall_area,
            expected_net_wall_area,
            1.0,
            "total wall area (net of openings)",
        );

        let roof_area: f64 = roofs.iter().map(|b| b.area_m2).sum();
        assert_within_pct(roof_area, ATTIC_ROOF_TOTAL_M2, 1.0, "roof area");

        let floor_area: f64 = floors.iter().map(|b| b.area_m2).sum();
        assert_within_pct(floor_area, ATTIC_FLOOR_M2, 1.0, "attic floor area");

        let slab_area: f64 = slabs.iter().map(|b| b.area_m2).sum();
        assert_within_pct(slab_area, SLAB_M2, 1.0, "slab area");

        let door_area: f64 = doors.iter().map(|b| b.area_m2).sum();
        assert_within_pct(door_area, DOOR_M2, 1.0, "door area");

        let window_area: f64 = windows.iter().map(|b| b.area_m2).sum();
        assert_within_pct(window_area, WINDOW_TOTAL_M2, 1.0, "window area");

        // Solar absorptance -- must be present on HPXML-parsed exterior walls.
        let living_walls: Vec<_> = hpxml_walls
            .iter()
            .filter(|b| b.interior_zone.as_ref() == Some(&ZoneType::Conditioned))
            .collect();
        for w in &living_walls {
            let abs = w.solar_absorptance.unwrap_or_else(|| {
                panic!(
                    "wall {} must have solar_absorptance parsed from HPXML",
                    w.id
                )
            });
            assert!(
                (abs - WALL_ABSORPTANCE).abs() < 0.01,
                "wall {} absorptance: got {abs}, expected {WALL_ABSORPTANCE}",
                w.id
            );
        }
        for r in &roofs {
            let abs = r.solar_absorptance.unwrap_or_else(|| {
                panic!(
                    "roof {} must have solar_absorptance parsed from HPXML",
                    r.id
                )
            });
            assert!(
                (abs - ROOF_ABSORPTANCE).abs() < 0.01,
                "roof {} absorptance: got {abs}, expected {ROOF_ABSORPTANCE}",
                r.id
            );
        }

        // Window properties -- must be present and match HPXML values
        for win in &building.windows {
            let u = win
                .u_factor_w_m2_k
                .unwrap_or_else(|| panic!("window {} must have u_factor_w_m2_k parsed", win.id));
            assert_within_pct(
                u,
                WINDOW_U_FACTOR_SI,
                2.0,
                &format!("window {} U-factor", win.id),
            );

            let shgc = win
                .shgc
                .unwrap_or_else(|| panic!("window {} must have SHGC parsed", win.id));
            assert!(
                (shgc - WINDOW_SHGC).abs() < 0.01,
                "window {} SHGC: got {shgc}, expected {WINDOW_SHGC}",
                win.id
            );
        }

        eprintln!("[structural] beopt_building_structure: all assertions passed");
    }

    // ── Test 2: Conversion correctness ───────────────────────────────────

    #[test]
    fn beopt_boundary_inputs() {
        let building = parse_hpxml(&beopt_xml_path()).expect("parse BEopt HPXML");
        let defaults_path = project_root().join("defaults");
        let defaults = DefaultsStore::load(&defaults_path).expect("load defaults");

        let n_zones = building.zones.len();
        let zone_inputs = building_to_zone_inputs(&building, n_zones);
        let boundary_inputs =
            building_to_boundary_inputs(&building, n_zones, &defaults, 2.0, 10.0, 10.0);

        assert_eq!(
            zone_inputs.len(),
            n_zones,
            "zone_inputs length must match n_zones"
        );
        assert!(
            !boundary_inputs.is_empty(),
            "boundary_inputs must not be empty"
        );

        // All interior_zone_idx must be in range
        for (i, bi) in boundary_inputs.iter().enumerate() {
            assert!(
                bi.interior_zone_idx < n_zones,
                "boundary {i} interior_zone_idx={} out of range (n_zones={n_zones})",
                bi.interior_zone_idx
            );
        }

        // Exterior walls → ExteriorTarget::Outdoor
        let outdoor_walls: Vec<_> = boundary_inputs
            .iter()
            .enumerate()
            .filter(|(_, bi)| bi.exterior == ExteriorTarget::Outdoor && bi.area_m2 > 10.0)
            .collect();
        assert!(
            !outdoor_walls.is_empty(),
            "must have at least one outdoor-facing wall boundary"
        );

        // Attic floor → ExteriorTarget::Zone (attic zone index)
        let attic_zone_idx = zone_index(&building, ZoneType::Attic);
        let zone_coupled: Vec<_> = boundary_inputs
            .iter()
            .filter(|bi| bi.exterior == ExteriorTarget::Zone(attic_zone_idx))
            .collect();
        assert!(
            !zone_coupled.is_empty(),
            "must have at least one boundary coupled to attic zone (idx={attic_zone_idx})"
        );

        // At least some boundaries should have precomputed RC layers (LUT hit)
        let lut_hits: usize = boundary_inputs
            .iter()
            .filter(|bi| !bi.precomputed_rc.is_empty())
            .count();
        eprintln!(
            "[structural] LUT hits: {lut_hits}/{} boundaries",
            boundary_inputs.len()
        );
        assert!(
            lut_hits > 0,
            "at least one boundary must resolve to precomputed RC layers from envelope LUT"
        );

        // Exterior target distribution
        let mut outdoor_count = 0;
        let mut ground_count = 0;
        let mut zone_count = 0;
        for bi in &boundary_inputs {
            match bi.exterior {
                ExteriorTarget::Outdoor => outdoor_count += 1,
                ExteriorTarget::Ground => ground_count += 1,
                ExteriorTarget::Zone(_) => zone_count += 1,
            }
        }
        eprintln!(
            "[structural] boundary targets: outdoor={outdoor_count}, ground={ground_count}, zone={zone_count}"
        );

        assert!(
            ground_count >= 1,
            "slab must connect to ExteriorTarget::Ground, got ground_count={ground_count}"
        );
    }

    // ── Test 3: RC network topology ──────────────────────────────────────

    #[test]
    fn beopt_rc_network_topology() {
        let building = parse_hpxml(&beopt_xml_path()).expect("parse BEopt HPXML");
        let defaults_path = project_root().join("defaults");
        let defaults = DefaultsStore::load(&defaults_path).expect("load defaults");

        let n_zones = building.zones.len();
        let zone_inputs = building_to_zone_inputs(&building, n_zones);
        let boundary_inputs =
            building_to_boundary_inputs(&building, n_zones, &defaults, 2.0, 10.0, 10.0);
        let zone_caps = derive_zone_capacitances(&zone_inputs, site_pressure_for_building(&building));

        assert_eq!(zone_caps.len(), n_zones, "zone capacitances length");

        // Look up zone indices by type, not hardcoded position
        let cond_idx = zone_index(&building, ZoneType::Conditioned);
        let attic_idx = zone_index(&building, ZoneType::Attic);

        // Conditioned zone capacitance: C = rho_air * cp_air * V * TCM.
        // BEopt HPXML generates furniture boundaries (conditioned_furniture) which
        // provide explicit RC thermal-mass nodes equivalent to E+ InternalMass
        // objects. Per E+ convention, ZoneCapacitanceMultiplier and InternalMass are
        // mutually exclusive, so TCM = 1.0 (air capacitance only) when furniture
        // boundaries are present. This avoids double-counting the ~39% overstating
        // that occurs when both the 7.0 multiplier and furniture RC nodes are used.
        // See: building_to_zone_inputs (conversions.rs); E+ InputOutputRef.
        let hares_indoor_cap = 1.2
            * 1006.0
            * OCHRE_INDOOR_VOLUME_M3
            * 1.0; // furniture boundaries present → TCM = 1.0
        assert_within_pct(
            zone_caps[cond_idx],
            hares_indoor_cap,
            5.0,
            "indoor zone capacitance",
        );

        // Attic zone capacitance uses TCM = 1.0 (air only: unconditioned attics have
        // no furniture boundaries). OCHRE applies x7 uniformly, which
        // overstates attic inertia ~7x. HARES attic mass = air only; see
        // `mass_multiplier_for_zone` in hares-core/src/dwelling/conversions.rs.
        let hares_attic_cap =
            1.2 * 1006.0 * OCHRE_ATTIC_VOLUME_M3 * mass_multiplier_for_zone(&ZoneType::Attic);
        assert_within_pct(
            zone_caps[attic_idx],
            hares_attic_cap,
            5.0,
            "attic zone capacitance",
        );

        // Assemble the RC network
        let (rc, _diag) = assemble_building_rc(&boundary_inputs, n_zones, &zone_caps, InteriorLwrMethod::StarMesh)
            .expect("assemble_building_rc must succeed");

        // Zone state rows
        assert_eq!(
            rc.zone_state_rows.len(),
            n_zones,
            "zone_state_rows.len() must equal n_zones"
        );

        // Outdoor column present
        assert!(
            rc.outdoor_col.is_some(),
            "outdoor_col must be Some -- building has outdoor-facing boundaries"
        );

        // State matrix must be square
        assert_eq!(
            rc.a_c.nrows(),
            rc.a_c.ncols(),
            "A_c must be square: {}x{}",
            rc.a_c.nrows(),
            rc.a_c.ncols()
        );

        let n_states = rc.a_c.nrows();

        // Must have more states than just zones (layer nodes exist)
        assert!(
            n_states > n_zones,
            "n_states ({n_states}) must exceed n_zones ({n_zones}) -- layer nodes should exist"
        );

        // All diagonal entries of A_c must be negative (stable dissipative system)
        for i in 0..n_states {
            assert!(
                rc.a_c[(i, i)] <= 0.0,
                "A_c diagonal [{i},{i}] = {} must be <= 0 for stability",
                rc.a_c[(i, i)]
            );
        }

        // layer_info non-empty (exterior surfaces have outer nodes)
        assert!(
            !rc.layer_info.is_empty(),
            "layer_info must be non-empty -- exterior surfaces need outer material nodes"
        );

        // B_ext rows must match state count
        assert_eq!(
            rc.b_ext.nrows(),
            n_states,
            "B_ext row count must match state count"
        );

        // External input columns: outdoor temp is a single driving signal shared across
        // all outdoor-facing boundaries, so n_ext=1 is expected even with many boundaries.
        assert!(
            rc.n_ext > 0,
            "n_ext must be > 0 -- building has external boundary conditions"
        );

        // Print topology summary
        eprintln!("\n=== RC Network Topology ===");
        eprintln!("  zones: {n_zones}");
        eprintln!("  total states: {n_states}");
        eprintln!("  layer nodes: {}", n_states - n_zones);
        eprintln!("  external inputs (n_ext): {}", rc.n_ext);
        eprintln!("  outdoor_col: {:?}", rc.outdoor_col);
        eprintln!("  layer_info entries: {}", rc.layer_info.len());
        eprintln!(
            "  zone capacitances [kJ/K]: {:?}",
            zone_caps.iter().map(|c| c / 1000.0).collect::<Vec<_>>()
        );
        eprintln!(
            "  OCHRE reference indoor cap: {:.0} J/K (HARES: {:.0})",
            OCHRE_INDOOR_CAPACITANCE_JK, zone_caps[cond_idx]
        );
        eprintln!(
            "  OCHRE reference attic cap:  {:.0} J/K (HARES: {:.0})",
            OCHRE_ATTIC_CAPACITANCE_JK, zone_caps[attic_idx]
        );

        eprintln!("[structural] beopt_rc_network_topology: all assertions passed");
    }

    // ── Test 4: Boundary-by-boundary UA diagnostics ────────────────────

    #[test]
    fn beopt_boundary_ua_diagnostics() {
        let building = parse_hpxml(&beopt_xml_path()).expect("parse BEopt HPXML");
        let defaults_path = project_root().join("defaults");
        let defaults = DefaultsStore::load(&defaults_path).expect("load defaults");

        let n_zones = building.zones.len();
        let zone_inputs = building_to_zone_inputs(&building, n_zones);
        let boundary_inputs =
            building_to_boundary_inputs(&building, n_zones, &defaults, 2.0, 10.0, 10.0);
        let zone_caps = derive_zone_capacitances(&zone_inputs, site_pressure_for_building(&building));

        let (_rc, diag) = assemble_building_rc(&boundary_inputs, n_zones, &zone_caps, InteriorLwrMethod::StarMesh)
            .expect("assemble_building_rc must succeed");

        eprintln!("\n=== Boundary-by-Boundary UA Diagnostics ===");
        eprintln!(
            "{:<4} {:<8} {:>10} {:>10} {:>10} {:>6} {:>6} {:<12} {:<10}",
            "idx", "path", "area_m2", "R_total", "UA_W/K", "nodes", "zone", "exterior", "C_kJ/K"
        );
        eprintln!("{}", "-".repeat(90));

        for d in &diag.boundaries {
            let path_str = match d.path {
                RCPath::Precomputed => "LUT",
                RCPath::MaterialLayer => "layers",
                RCPath::FallbackR => "fallbk",
            };
            let ext_str = match d.exterior_target {
                ExteriorTarget::Outdoor => "Outdoor".to_string(),
                ExteriorTarget::Ground => "Ground".to_string(),
                ExteriorTarget::Zone(i) => format!("Zone({i})"),
            };
            eprintln!(
                "{:<4} {:<8} {:>10.2} {:>10.4} {:>10.2} {:>6} {:>6} {:<12} {:>10.2}",
                d.boundary_idx,
                path_str,
                d.area_m2,
                d.r_total_m2_k_w,
                d.ua_w_per_k,
                d.n_rc_nodes,
                d.interior_zone_idx,
                ext_str,
                d.capacitance_j_k / 1000.0,
            );
        }

        eprintln!("{}", "-".repeat(90));
        eprintln!("Total UA: {:.2} W/K", diag.total_ua_w_per_k);
        eprintln!(
            "Zone capacitances [kJ/K]: {:?}",
            diag.zone_capacitances_j_k
                .iter()
                .map(|c| c / 1000.0)
                .collect::<Vec<_>>()
        );
        eprintln!("OCHRE effective total UA: ~559 W/K (with film + halving)");

        // LUT resolution per boundary
        let lut = defaults
            .envelope_lut()
            .expect("envelope LUT must be loaded");
        eprintln!("\n=== LUT Resolution per Boundary ===");
        for bd in &building.boundaries {
            let name = bd.lut_boundary_name.as_deref().or_else(|| {
                resolve_boundary_name(
                    &bd.boundary_type,
                    bd.interior_zone.as_ref(),
                    bd.exterior_zone.as_ref(),
                )
            });
            let r_value = bd.assembly_r_value_m2_k_w.or_else(|| {
                let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                if sum > 0.0 { Some(sum) } else { None }
            });
            let result = name.and_then(|n| {
                lut.lookup(
                    n,
                    bd.construction_type.as_deref(),
                    bd.finish_type.as_deref(),
                    bd.insulation_details.as_deref(),
                    r_value,
                )
            });
            let (matched_type, matched_r) = match &result {
                Some(r) => (r.matched_boundary_type.as_str(), r.matched_r_value),
                None => ("MISS", 0.0),
            };
            eprintln!(
                "  {} ({:?}) → name={:?} ct={:?} ft={:?} ins={:?} r={:?} → {} R={:.4}",
                bd.id,
                bd.boundary_type,
                name,
                bd.construction_type,
                bd.finish_type,
                bd.insulation_details,
                r_value,
                matched_type,
                matched_r,
            );
        }
        eprintln!(
            "Boundaries: {} total, {} LUT, {} layers, {} fallback",
            diag.boundaries.len(),
            diag.boundaries
                .iter()
                .filter(|d| d.path == RCPath::Precomputed)
                .count(),
            diag.boundaries
                .iter()
                .filter(|d| d.path == RCPath::MaterialLayer)
                .count(),
            diag.boundaries
                .iter()
                .filter(|d| d.path == RCPath::FallbackR)
                .count(),
        );

        assert!(
            !diag.boundaries.is_empty(),
            "diagnostics must contain at least one boundary"
        );
        assert!(
            diag.total_ua_w_per_k > 0.0,
            "total UA must be positive, got {}",
            diag.total_ua_w_per_k
        );
        for d in &diag.boundaries {
            assert!(
                d.area_m2 > 0.0,
                "boundary {} area must be > 0",
                d.boundary_idx
            );
            assert!(
                d.r_total_m2_k_w > 0.0,
                "boundary {} R_total must be > 0, got {}",
                d.boundary_idx,
                d.r_total_m2_k_w
            );
            assert!(
                d.ua_w_per_k > 0.0,
                "boundary {} UA must be > 0, got {}",
                d.boundary_idx,
                d.ua_w_per_k
            );
        }
    }

    // ── Test 5: UA parity with OCHRE ────────────────────────────────────

    #[test]
    fn beopt_ua_parity() {
        let building = parse_hpxml(&beopt_xml_path()).expect("parse BEopt HPXML");
        let defaults_path = project_root().join("defaults");
        let defaults = DefaultsStore::load(&defaults_path).expect("load defaults");

        let n_zones = building.zones.len();
        let zone_inputs = building_to_zone_inputs(&building, n_zones);
        let boundary_inputs =
            building_to_boundary_inputs(&building, n_zones, &defaults, 2.0, 10.0, 10.0);
        let zone_caps = derive_zone_capacitances(&zone_inputs, site_pressure_for_building(&building));

        let (_rc, diag) = assemble_building_rc(&boundary_inputs, n_zones, &zone_caps, InteriorLwrMethod::StarMesh)
            .expect("assemble_building_rc must succeed");

        // Load ASHRAE-correct RC reference. Interior film R is convection-only
        // (TARP h_conv, no h_rad). Longwave radiation is handled by the
        // explicit interior LWR exchange module (ScriptF surface-to-surface).
        // Window U-factor converts IP BTU/(hr*ft2*F) -> SI via x5.678 per
        // ASHRAE 90.1-2022.
        let ref_json_path = project_root().join("tests/fixtures/parity/ashrae_rc_reference.json");
        let ref_json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&ref_json_path)
                .unwrap_or_else(|e| panic!("read {}: {e}", ref_json_path.display())),
        )
        .expect("parse ashrae_rc_reference.json");
        let ochre_boundaries = ref_json["boundaries"].as_array().expect("boundaries array");
        let ochre_total_ua = ref_json["total_ua_w_k"].as_f64().unwrap_or(0.0);

        // Aggregate HARES boundary diagnostics by LUT name
        // (OCHRE consolidates walls by type; HARES keeps per-orientation).
        struct AggregatedBoundary {
            ua_w_k: f64,
            area_m2: f64,
            r_film_int_sum: f64,
            r_film_ext_sum: f64,
            capacitance_j_k: f64,
            n_rc_nodes: usize,
            count: usize,
        }
        let mut hares_by_name: std::collections::HashMap<String, AggregatedBoundary> =
            std::collections::HashMap::new();
        for (bd, diag_bd) in building.boundaries.iter().zip(diag.boundaries.iter()) {
            let name = bd
                .lut_boundary_name
                .clone()
                .or_else(|| {
                    resolve_boundary_name(
                        &bd.boundary_type,
                        bd.interior_zone.as_ref(),
                        bd.exterior_zone.as_ref(),
                    )
                    .map(|s| s.to_string())
                })
                .unwrap_or_else(|| format!("Unknown({})", bd.id));
            let entry = hares_by_name.entry(name).or_insert(AggregatedBoundary {
                ua_w_k: 0.0,
                area_m2: 0.0,
                r_film_int_sum: 0.0,
                r_film_ext_sum: 0.0,
                capacitance_j_k: 0.0,
                n_rc_nodes: 0,
                count: 0,
            });
            entry.ua_w_k += diag_bd.ua_w_per_k;
            entry.area_m2 += diag_bd.area_m2;
            entry.r_film_int_sum += diag_bd.r_film_int_m2_k_w * diag_bd.area_m2;
            entry.r_film_ext_sum += diag_bd.r_film_ext_m2_k_w * diag_bd.area_m2;
            entry.capacitance_j_k += diag_bd.capacitance_j_k;
            entry.n_rc_nodes += diag_bd.n_rc_nodes;
            entry.count += 1;
        }

        // Compare each OCHRE reference boundary against HARES
        let pct = |h: f64, o: f64| -> f64 {
            if o.abs() > 1e-9 {
                (h - o) / o * 100.0
            } else if h.abs() > 1e-9 {
                f64::INFINITY
            } else {
                0.0
            }
        };

        eprintln!("\n{:=^120}", " RC PARITY: HARES vs OCHRE (per-boundary) ");
        eprintln!(
            "{:<20} {:>8} {:>8} {:>6}  {:>8} {:>8} {:>6}  {:>8} {:>8} {:>6}  {:>7} {:>7} {:>6}  {:>3} {:>3}",
            "Boundary",
            "H_UA",
            "O_UA",
            "Δ%",
            "H_Ri",
            "O_Ri",
            "Δ%",
            "H_Re",
            "O_Re",
            "Δ%",
            "H_Cap",
            "O_Cap",
            "Δ%",
            "H_n",
            "O_n"
        );
        eprintln!("{}", "-".repeat(120));

        let mut failures: Vec<String> = Vec::new();
        let mut matched_count = 0usize;

        for ochre_bd in ochre_boundaries {
            let name = ochre_bd["name"].as_str().unwrap_or("?");
            let o_ua = ochre_bd["ua_w_k"].as_f64().unwrap_or(0.0);
            let o_area = ochre_bd["area_m2"].as_f64().unwrap_or(0.0);
            let o_ri = ochre_bd["r_film_int_m2_k_w"].as_f64().unwrap_or(0.0);
            let o_re = ochre_bd["r_film_ext_m2_k_w"].as_f64().unwrap_or(0.0);
            let o_cap_kj = ochre_bd["capacitance_kj_k"].as_f64().unwrap_or(0.0);
            let o_nodes = ochre_bd["n_nodes"].as_u64().unwrap_or(0) as usize;

            // Windows are stored per-window in HARES (Window1 … WindowN) but
            // aggregated under the single "Window" name in the reference.
            // The dedicated aggregate-window comparison below handles them,
            // so skip the direct lookup here.
            if name == "Window" {
                continue;
            }

            let Some(h) = hares_by_name.get(name) else {
                eprintln!("{:<20} -- NO MATCH IN HARES --", name);
                failures.push(format!("{name}: no matching HARES boundary"));
                continue;
            };
            matched_count += 1;

            // Area-weighted average film R for multi-segment boundaries
            let h_ri = if h.area_m2 > 0.0 {
                h.r_film_int_sum / h.area_m2
            } else {
                0.0
            };
            let h_re = if h.area_m2 > 0.0 {
                h.r_film_ext_sum / h.area_m2
            } else {
                0.0
            };
            let h_cap_kj = h.capacitance_j_k / 1000.0;

            let ua_pct = pct(h.ua_w_k, o_ua);
            let ri_pct = pct(h_ri, o_ri);
            let re_pct = pct(h_re, o_re);
            let cap_pct = pct(h_cap_kj, o_cap_kj);

            let flag = |p: f64, tol: f64| -> &str { if p.abs() <= tol { " " } else { "!" } };

            eprintln!(
                "{:<20} {:>8.2} {:>8.2} {:>+5.1}%{} {:>8.4} {:>8.4} {:>+5.1}%{} {:>8.4} {:>8.4} {:>+5.1}%{} {:>7.1} {:>7.1} {:>+5.1}%{} {:>3} {:>3} {}",
                name,
                h.ua_w_k,
                o_ua,
                ua_pct,
                flag(ua_pct, 1.0),
                h_ri,
                o_ri,
                ri_pct,
                flag(ri_pct, 5.0),
                h_re,
                o_re,
                re_pct,
                flag(re_pct, 5.0),
                h_cap_kj,
                o_cap_kj,
                cap_pct,
                flag(cap_pct, 10.0),
                h.n_rc_nodes,
                o_nodes,
                if h.n_rc_nodes == o_nodes { " " } else { "!" },
            );

            // UA within 1% (existing tolerance)
            if o_ua >= 1.0 && ua_pct.abs() > 1.0 {
                failures.push(format!(
                    "{name} UA: {:.2} vs {:.2} ({:+.1}%)",
                    h.ua_w_k, o_ua, ua_pct
                ));
            }
            // Film R_int within 5% or 0.01
            if o_ri > 0.01 && ri_pct.abs() > 5.0 && (h_ri - o_ri).abs() > 0.01 {
                failures.push(format!(
                    "{name} R_film_int: {:.4} vs {:.4} ({:+.1}%)",
                    h_ri, o_ri, ri_pct
                ));
            }
            // Film R_ext within 5% or 0.01
            if o_re > 0.01 && re_pct.abs() > 5.0 && (h_re - o_re).abs() > 0.01 {
                failures.push(format!(
                    "{name} R_film_ext: {:.4} vs {:.4} ({:+.1}%)",
                    h_re, o_re, re_pct
                ));
            }
            // Capacitance within 10% or 50 kJ/K
            if o_cap_kj > 10.0 && cap_pct.abs() > 10.0 && (h_cap_kj - o_cap_kj).abs() > 50.0 {
                failures.push(format!(
                    "{name} capacitance: {:.1} vs {:.1} kJ/K ({:+.1}%)",
                    h_cap_kj, o_cap_kj, cap_pct
                ));
            }
            // Node count exact
            if h.n_rc_nodes != o_nodes {
                failures.push(format!("{name} n_nodes: {} vs {}", h.n_rc_nodes, o_nodes));
            }
            // Area within 1%
            let area_pct = pct(h.area_m2, o_area);
            if o_area > 0.1 && area_pct.abs() > 1.0 {
                failures.push(format!(
                    "{name} area: {:.2} vs {:.2} m² ({:+.1}%)",
                    h.area_m2, o_area, area_pct
                ));
            }
        }

        // Total UA
        let hares_total = diag.total_ua_w_per_k;
        let total_pct = pct(hares_total, ochre_total_ua);
        eprintln!("{}", "-".repeat(120));
        eprintln!(
            "{:<20} {:>8.2} {:>8.2} {:>+5.1}%",
            "TOTAL UA", hares_total, ochre_total_ua, total_pct
        );

        // Aggregate unmatched HARES window boundaries and compare to OCHRE "Window"
        let hares_window_ua: f64 = hares_by_name
            .iter()
            .filter(|(name, _)| name.starts_with("Unknown(Window"))
            .map(|(_, agg)| agg.ua_w_k)
            .sum();
        let hares_window_area: f64 = hares_by_name
            .iter()
            .filter(|(name, _)| name.starts_with("Unknown(Window"))
            .map(|(_, agg)| agg.area_m2)
            .sum();
        let ochre_window = ochre_boundaries
            .iter()
            .find(|b| b["name"].as_str() == Some("Window"));
        if let Some(ow) = ochre_window {
            let o_win_ua = ow["ua_w_k"].as_f64().unwrap_or(0.0);
            let o_win_area = ow["area_m2"].as_f64().unwrap_or(0.0);
            let hares_u = if hares_window_area > 0.0 {
                hares_window_ua / hares_window_area
            } else {
                0.0
            };
            let ochre_u = if o_win_area > 0.0 {
                o_win_ua / o_win_area
            } else {
                0.0
            };
            eprintln!("\n  Window comparison (aggregated):");
            eprintln!(
                "    HARES: UA={:.2} W/K, area={:.2} m², U={:.3} W/(m²·K)",
                hares_window_ua, hares_window_area, hares_u
            );
            eprintln!(
                "    OCHRE: UA={:.2} W/K, area={:.2} m², U={:.3} W/(m²·K)",
                o_win_ua, o_win_area, ochre_u
            );
            eprintln!(
                "    Delta UA: {:.2} W/K ({:+.1}%)",
                hares_window_ua - o_win_ua,
                pct(hares_window_ua, o_win_ua)
            );
            eprintln!(
                "    NOTE: OCHRE uses raw HPXML UFactor={:.2} as SI W/(m²·K).",
                ochre_u
            );
            eprintln!(
                "          HPXML UFactor is in BTU/(hr·ft²·°F). Correct SI = {:.2} × 5.678 = {:.3} W/(m²·K).",
                ochre_u,
                ochre_u * 5.678
            );
            eprintln!(
                "          HARES correctly converts: U={:.3} W/(m²·K). OCHRE window R is {:.1}× too high.",
                hares_u,
                (1.0 / ochre_u) / (1.0 / hares_u)
            );
            // Area should match within 1%
            let area_pct = pct(hares_window_area, o_win_area);
            if o_win_area > 0.1 && area_pct.abs() > 1.0 {
                failures.push(format!(
                    "Window area: {:.2} vs {:.2} m² ({:+.1}%)",
                    hares_window_area, o_win_area, area_pct
                ));
            }
            // Aggregate UA must match within 1% (same tolerance as named
            // boundaries) -- this is the enforcement gate for the IP-U ->
            // SI conversion and Simple Glazing Model decomposition.
            let ua_pct = pct(hares_window_ua, o_win_ua);
            if o_win_ua > 1.0 && ua_pct.abs() > 1.0 {
                failures.push(format!(
                    "Window UA: {:.2} vs {:.2} W/K ({:+.1}%)",
                    hares_window_ua, o_win_ua, ua_pct
                ));
            }
            matched_count += 1;
        }

        // Remaining unmatched HARES boundaries (non-window)
        for (name, agg) in &hares_by_name {
            if name.starts_with("Unknown(Window") {
                continue; // already handled above
            }
            let matched = ochre_boundaries
                .iter()
                .any(|b| b["name"].as_str() == Some(name.as_str()));
            if !matched {
                eprintln!(
                    "  WARNING: HARES boundary '{}' (UA={:.2}, area={:.2}) has no OCHRE match",
                    name, agg.ua_w_k, agg.area_m2
                );
            }
        }

        eprintln!(
            "\n  Matched: {}/{} OCHRE boundaries",
            matched_count,
            ochre_boundaries.len()
        );

        assert_within_pct(hares_total, ochre_total_ua, 3.0, "total building UA");

        // Zone capacitances within 1%. Conditioned zone TCM = 1.0 because
        // furniture boundaries are present (see building_to_zone_inputs);
        // attic TCM = 1.0 (air only). OCHRE's uniform x7 over-counts both.
        let cond_idx = zone_index(&building, ZoneType::Conditioned);
        let attic_idx = zone_index(&building, ZoneType::Attic);
        let hares_indoor_cap = 1.2
            * 1006.0
            * OCHRE_INDOOR_VOLUME_M3
            * 1.0; // furniture boundaries present → TCM = 1.0
        assert_within_pct(
            zone_caps[cond_idx],
            hares_indoor_cap,
            1.0,
            "indoor zone capacitance",
        );
        let hares_attic_cap =
            1.2 * 1006.0 * OCHRE_ATTIC_VOLUME_M3 * mass_multiplier_for_zone(&ZoneType::Attic);
        assert_within_pct(
            zone_caps[attic_idx],
            hares_attic_cap,
            1.0,
            "attic zone capacitance",
        );

        // Report all failures at the end for visibility.  The per-boundary
        // checks above accumulate every divergence; any nonzero count means
        // HARES disagrees with the independent ASHRAE reference beyond the
        // documented tolerance.
        if !failures.is_empty() {
            eprintln!("\n  RC PARITY FAILURES ({}):", failures.len());
            for f in &failures {
                eprintln!("    - {f}");
            }
            panic!("{} RC parity failures", failures.len());
        }
    }

    // ── Test 6: Interior film resistance regression ─────────────────
    //
    // Locks in the convection-only interior film resistance convention.
    // R_film_int = 1/h_conv (TARP natural convection only). Longwave
    // radiation is handled entirely by the explicit interior LWR exchange
    // module (ScriptF surface-to-surface), not by the linearized h_rad
    // in the film coefficient. This matches EnergyPlus Eng.Ref "Inside
    // Heat Balance" which explicitly separates q''_conv (h_c only) from
    // q''_LWX (surface-to-surface LWR).
    //
    // Reference values (TARP h_conv for vertical wall with ΔT=12.9°C):
    //   h_conv = 1.31 × 12.9^(1/3) ≈ 3.076 W/(m²·K)
    //   R_film_int = 1/h_conv ≈ 0.325 m²·K/W
    //
    // ASHRAE Handbook of Fundamentals 2021, Ch. 26 Table 1 gives the
    // COMBINED film resistance (convection + radiation) as 0.120 m²·K/W
    // for vertical walls. The convection-only portion is ~0.325 m²·K/W.
    // The radiative portion (h_rad ≈ 5.14 W/(m²·K)) is handled by the
    // interior LWR module, not by R_film_int.
    #[test]
    fn ashrae_interior_film_resistance_regression() {
        use hares_physics::film_coefficients::{SurfaceRoughness, ZoneLabel, film_resistances};

        // Vertical wall, Conditioned interior, Outdoor exterior.
        // Convection-only R_film_int ≈ 0.325 m²·K/W for vertical wall.
        let (r_int_wall, _) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::MediumRough,
        );
        let h_conv = 1.31 * 12.9_f64.cbrt();
        let r_conv_only = 1.0 / h_conv;
        assert!(
            (r_int_wall - r_conv_only).abs() < 0.01,
            "vertical wall R_i={r_int_wall:.4} must equal 1/h_conv ≈ {r_conv_only:.4} \
             (convection-only from TARP with ΔT=12.9°C). \
             LWR is handled by the interior exchange module, not R_film."
        );

        // Ceiling from below (Conditioned looking up at Attic boundary -- heat
        // flow upward).  Convection-only R_film for horizontal, above_hotter=true.
        let (r_int_ceiling, _) = film_resistances(
            0.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Attic,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::MediumRough,
        );
        assert!(
            r_int_ceiling > 0.1 && r_int_ceiling < 0.7,
            "ceiling (heat flow up) R_i={r_int_ceiling:.4} out of convection-only range \
             [0.1, 0.7] m²·K/W."
        );

        // Floor from above (Conditioned looking down at Ground -- heat flow
        // down, stable).  Convection-only R_film for horizontal, above_hotter=false.
        let (r_int_floor, _) = film_resistances(
            0.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Ground,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::MediumRough,
        );
        assert!(
            r_int_floor > 0.1 && r_int_floor < 0.5,
            "floor (heat flow down) R_i={r_int_floor:.4} out of convection-only range \
             [0.1, 0.5] m²·K/W."
        );

        // Regression guard: R_film_int MUST be convection-only.
        // The old combined R_film ≈ 0.120 violated this by baking h_rad
        // into the film resistance, causing double-counting with the
        // interior LWR module.
        assert!(
            r_int_wall > 0.25,
            "vertical wall R_i={r_int_wall:.4} < 0.25 -- appears to include h_rad \
             (combined film regression). Interior film must be convection-only; \
             LWR is handled by the explicit interior exchange module."
        );
    }
}
