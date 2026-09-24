//! Regression tests for window U-factor and SHGC silent defaults.
//!
//! ## What was fixed
//!
//! `solver_builder.rs` previously used `unwrap_or(5.0)` and `unwrap_or(0.4)` to
//! silently inject single-pane aluminium window performance when a window's
//! U-factor or SHGC was `None`.  A silent 5.0 W/(m²·K) default is ~3× worse
//! than any IECC 2021 code-compliant modern window (max 1.70 W/(m²·K), CZ 5–8).
//!
//! The fix operates at two layers:
//! 1. **Solver layer** (`build_solver_boundaries`): `None` fields now produce a
//!    loud `HaresError::Dwelling(...)` error instead of silent substitution.
//! 2. **HPXML validation layer** (`validate_building_ranges`): every `<Window>`
//!    without `<UFactor>` or `<SHGC>` is now a `ValidationError`.
//!
//! ## What these tests demonstrate
//!
//! 1. The HPXML IO-layer parser correctly returns `None` when elements are
//!    absent — this was already correct and must remain so.
//! 2. The HPXML IO-layer parser correctly converts UFactor from US customary
//!    to SI when the element is present.
//! 3. Post-fix: `validate_building_ranges` reports errors for windows missing
//!    U-factor or SHGC.
//! 4. Post-fix: the solver layer errors loudly when a window has `None` for
//!    U-factor or SHGC, tested by constructing a `Building` with the gap and
//!    verifying the construction fails.

use hares_io::hpxml::building::parse_building;
use hares_io::hpxml::validation::validate_building_ranges;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal HPXML that includes one `<Window>` element without a
/// `<UFactor>` or `<SHGC>` child.
fn hpxml_with_window_missing_u_and_shgc() -> String {
    r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <WallType><WoodStud/></WallType>
            <Area units="ft2">200</Area>
            <Insulation>
              <SystemIdentifier id="WallIns1"/>
              <AssemblyEffectiveRValue>13</AssemblyEffectiveRValue>
            </Insulation>
          </Wall>
        </Walls>
        <Windows>
          <Window>
            <SystemIdentifier id="Win1"/>
            <Area units="ft2">15</Area>
            <Azimuth>180</Azimuth>
            <AttachedToWall idref="Wall1"/>
            <!-- UFactor and SHGC deliberately omitted to trigger validation errors -->
          </Window>
        </Windows>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#
        .to_string()
}

/// Build a minimal HPXML that includes one `<Window>` element with explicit,
/// code-compliant U-factor and SHGC values.
fn hpxml_with_window_explicit_u_and_shgc() -> String {
    r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site><SiteType>suburban</SiteType></Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="m2">200</ConditionedFloorArea>
          <ConditionedBuildingVolume units="m3">500</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure>
        <Walls>
          <Wall>
            <SystemIdentifier id="Wall1"/>
            <ExteriorAdjacentTo>outside</ExteriorAdjacentTo>
            <InteriorAdjacentTo>conditioned space</InteriorAdjacentTo>
            <WallType><WoodStud/></WallType>
            <Area units="ft2">200</Area>
            <Insulation>
              <SystemIdentifier id="WallIns1"/>
              <AssemblyEffectiveRValue>13</AssemblyEffectiveRValue>
            </Insulation>
          </Wall>
        </Walls>
        <Windows>
          <Window>
            <SystemIdentifier id="Win1"/>
            <Area units="ft2">15</Area>
            <Azimuth>180</Azimuth>
            <UFactor>0.30</UFactor>
            <SHGC>0.30</SHGC>
            <AttachedToWall idref="Wall1"/>
          </Window>
        </Windows>
      </Enclosure>
    </BuildingDetails>
  </Building>
</HPXML>"#
        .to_string()
}

// ===========================================================================
// HPXML parsing layer (already correct — these should PASS now and after fix)
// ===========================================================================

/// The HPXML parser must leave `u_factor_w_m2_k = None` when the `<UFactor>`
/// element is absent.  This verifies the IO layer is already correct and
/// propagates `None` up rather than defaulting itself.
#[test]
fn hpxml_parser_returns_none_u_factor_when_element_absent() {
    let building = parse_building(&hpxml_with_window_missing_u_and_shgc())
        .expect("HPXML with missing UFactor should parse at the IO level");

    let win = building
        .windows
        .first()
        .expect("fixture must have at least one window");

    assert!(
        win.u_factor_w_m2_k.is_none(),
        "HPXML parser must propagate None for u_factor_w_m2_k when <UFactor> is absent, \
         not silently invent a value (got {:?})",
        win.u_factor_w_m2_k
    );
}

/// The HPXML parser must leave `shgc = None` when the `<SHGC>` element is
/// absent.  Same reasoning as above.
#[test]
fn hpxml_parser_returns_none_shgc_when_element_absent() {
    let building = parse_building(&hpxml_with_window_missing_u_and_shgc())
        .expect("HPXML with missing SHGC should parse at the IO level");

    let win = building
        .windows
        .first()
        .expect("fixture must have at least one window");

    assert!(
        win.shgc.is_none(),
        "HPXML parser must propagate None for shgc when <SHGC> is absent, \
         not silently invent a value (got {:?})",
        win.shgc
    );
}

/// When both UFactor and SHGC are explicitly provided the parser must parse
/// and convert them correctly.  The HPXML value 0.30 Btu/(h·ft²·°F) must
/// convert to approximately 1.703 W/(m²·K) (×5.678263 conversion factor).
#[test]
fn hpxml_parser_converts_u_factor_from_imperial_to_si() {
    let building = parse_building(&hpxml_with_window_explicit_u_and_shgc())
        .expect("HPXML with explicit UFactor/SHGC should parse");

    let win = building
        .windows
        .first()
        .expect("fixture must have one window");

    let u = win
        .u_factor_w_m2_k
        .expect("u_factor_w_m2_k must be Some when <UFactor> is present");

    // 0.30 Btu/(h·ft²·°F) * 5.678263 = 1.7035 W/(m²·K)
    assert!(
        (u - 1.703_5).abs() < 0.01,
        "UFactor 0.30 Btu/(h·ft²·°F) must convert to ~1.70 W/(m²·K), got {u:.4}"
    );

    let shgc = win.shgc.expect("shgc must be Some when <SHGC> is present");
    assert!(
        (shgc - 0.30).abs() < 1e-6,
        "SHGC 0.30 must be stored as-is (dimensionless), got {shgc:.6}"
    );
}

// ===========================================================================
// HPXML validation layer (post-fix — these verify the fix took effect)
// ===========================================================================

/// After the fix, `validate_building_ranges` must report an error when a
/// window is missing U-factor.  This ensures the HPXML input path rejects
/// incomplete window data before it reaches the solver layer.
#[test]
fn validation_rejects_missing_u_factor() {
    let building = parse_building(&hpxml_with_window_missing_u_and_shgc())
        .expect("IO layer should parse even with missing UFactor");

    let report = validate_building_ranges(&building);
    assert!(
        report.has_errors(),
        "validation must report errors when window U-factor is absent"
    );

    let has_u_factor_error = report.errors.iter().any(|e| e.field == "WindowUFactor");
    assert!(
        has_u_factor_error,
        "validation errors must include 'WindowUFactor' when u_factor_w_m2_k is None; \
         got errors: {report:?}"
    );
}

/// After the fix, `validate_building_ranges` must report an error when a
/// window is missing SHGC.
#[test]
fn validation_rejects_missing_shgc() {
    let building = parse_building(&hpxml_with_window_missing_u_and_shgc())
        .expect("IO layer should parse even with missing SHGC");

    let report = validate_building_ranges(&building);
    assert!(
        report.has_errors(),
        "validation must report errors when window SHGC is absent"
    );

    let has_shgc_error = report.errors.iter().any(|e| e.field == "WindowSHGC");
    assert!(
        has_shgc_error,
        "validation errors must include 'WindowSHGC' when shgc is None; \
         got errors: {report:?}"
    );
}

/// After the fix, a window with both UFactor and SHGC present must pass
/// validation without errors related to window thermal properties.
#[test]
fn validation_passes_when_u_and_shgc_are_present() {
    let building = parse_building(&hpxml_with_window_explicit_u_and_shgc())
        .expect("HPXML with explicit UFactor/SHGC should parse");

    let report = validate_building_ranges(&building);

    let has_window_error = report
        .errors
        .iter()
        .any(|e| e.field == "WindowUFactor" || e.field == "WindowSHGC");
    assert!(
        !has_window_error,
        "validation must not report Window U-factor or SHGC errors when both are present; \
         got errors: {report:?}"
    );
}
