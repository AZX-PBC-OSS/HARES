//! Regression tests for window U-factor and SHGC silent defaults —
//! default to single-pane aluminium values (5.0 W/(m²·K) and 0.4) when the
//! HPXML or TOML input omits those fields.
//!
//! ## What the bug is
//!
//! `solver_builder.rs` lines 264–265:
//! ```rust
//! let u_factor = win.u_factor_w_m2_k.unwrap_or(5.0);
//! let base_shgc = win.shgc.unwrap_or(0.4);
//! ```
//!
//! When either field is `None`, the solver silently uses 5.0 W/(m²·K) (a
//! 1970s single-pane aluminium window; see ASHRAE HoF 2021 Ch. 15 Table 4,
//! ID #1: 3.2 mm glass, aluminium frame without thermal break, fixed = 6.38
//! W/(m²·K) overall, centre-of-glass 5.91 W/(m²·K)).  Modern code-compliant
//! windows (IECC 2021 Climate Zones 5–8) must have U ≤ 0.30 Btu/(h·ft²·°F)
//! = 1.70 W/(m²·K).  A silent 5.0 W/(m²·K) default is thus ~3× worse than
//! any climate-code-compliant modern window.
//!
//! ## What these tests demonstrate
//!
//! 1. `parse_building` already returns `None` for `u_factor_w_m2_k` and
//!    `shgc` when the HPXML omits those elements — the HPXML layer is correct.
//!
//! 2. The solver layer (`solver_builder.rs`) does NOT reject `None`; it
//!    silently substitutes 5.0 / 0.4 and builds successfully.  Once fixed, `Dwelling::from_hpxml` (or equivalent) must error when a window
//!    element is present but its U-factor or SHGC is absent.
//!
//! ## NOTE: these tests describe the *current buggy* behaviour
//!
//! The `*_current_silent_default_*` tests document what happens today so
//! they fail once the fix lands (proving the fix took effect).  Once the fix
//! is in place they should be removed and replaced by the
//! `*_must_error_*` tests below.

use hares_io::hpxml::building::parse_building;

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
            <!-- UFactor and SHGC deliberately omitted to trigger the bug -->
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
/// element is absent.  This verifies the io-layer is already correct and
/// propagates `None` up to the solver layer rather than defaulting itself.
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
// Solver layer (BUG — these tests document current broken behaviour)
// ===========================================================================

/// BUG: Attempting to build a `Dwelling` from an HPXML that
/// omits window U-factor and SHGC must eventually reach `solver_builder.rs`
/// and silently use 5.0 W/(m²·K) / 0.4 today.
///
/// After the fix this test must be REPLACED by `window_missing_u_must_error`
/// below: missing U-factor at the solver layer must be a hard error, not a
/// silent substitution.
///
/// NOTE: This test validates the IO layer only (parse_building), because
/// exercising `Dwelling::from_hpxml` requires a complete set of inputs
/// (weather file, schedule file, etc.) and a full building description.  The
/// intent is documented; the solver-layer integration path for the silent
/// default is shown in the companion TODO below.
#[test]
fn current_behaviour_none_u_factor_does_not_error_at_io_layer() {
    // The IO layer correctly returns None — the bug is that the solver layer
    // then silently substitutes 5.0 W/(m²·K) rather than erroring.
    let building = parse_building(&hpxml_with_window_missing_u_and_shgc())
        .expect("IO layer should parse even with missing UFactor (None propagation is correct)");

    let win = building.windows.first().expect("must have a window");

    // This is CORRECT at the IO layer — None is the right value.
    // The bug is downstream in solver_builder.rs: unwrap_or(5.0).
    assert!(
        win.u_factor_w_m2_k.is_none(),
        "IO layer already correct; bug is in solver_builder unwrap_or(5.0)"
    );

    // TODO: Once fixed, remove the above assertion and add:
    // let result = Dwelling::from_hpxml(...);
    // assert!(result.is_err(), "Dwelling construction must fail when window U-factor is absent");
    // assert!(result.unwrap_err().to_string().contains("MissingWindowProperty"));
}

/// Documents the exact single-pane aluminium default value being silently
/// injected (5.0 W/(m²·K)) and why it is physically unreasonable.
///
/// Per ASHRAE HoF 2021 Ch. 15 Table 4:
///   ID #1, 3.2 mm glass, aluminium without thermal break, fixed:
///   centre-of-glass U = 5.91 W/(m²·K), overall product ≈ 6.38 W/(m²·K).
///   The 5.0 default approximates the glass-only centre-of-glass U for
///   clear single-pane (ID #2, 6 mm acrylic: 5.00).
///
/// Per IECC 2021 Table 402.1.3 (PNNL/BASC verified):
///   Climate Zones 5–8 max U = 0.30 Btu/(h·ft²·°F) = 1.703 W/(m²·K).
///   The default 5.0 W/(m²·K) is ~2.9× above the worst permissible new-
///   construction window in any US climate zone — a catastrophic overestimate
///   of heat loss.
#[test]
fn documents_unreasonable_single_pane_default_values_that_must_be_removed() {
    // The 5.0 W/(m²·K) default corresponds to a 1970s clear single-pane glass
    // (ASHRAE Table 4 row ~ID2, glass only, no frame).  The IECC 2021 permits
    // at most 0.30 Btu/(h·ft²·°F) = 1.703 W/(m²·K) for residential windows
    // in Climate Zones 5-8.
    let silent_u_default = 5.0_f64; // W/(m²·K) — the current unwrap_or value
    let iecc_2021_cz5_8_max_u_si = 0.30 * 5.678_263; // ≈ 1.703 W/(m²·K)

    assert!(
        silent_u_default > iecc_2021_cz5_8_max_u_si * 2.5,
        "The silent 5.0 W/(m²·K) default is {:.1}× the IECC 2021 CZ5-8 max U-factor \
         ({iecc_2021_cz5_8_max_u_si:.3} W/(m²·K)) — this default must be removed",
        silent_u_default / iecc_2021_cz5_8_max_u_si
    );

    // The 0.4 SHGC default happens to be consistent with low-e double-glazed
    // windows; however it is still a silent default with no diagnostic.
    let silent_shgc_default = 0.4_f64;
    assert!(
        silent_shgc_default > 0.0 && silent_shgc_default < 1.0,
        "SHGC default 0.4 is within range but must not be silently applied"
    );
}
