//! ASHRAE Simple interior + DOE-2 exterior film coefficient calculations.
//!
//! Interior convection uses the ASHRAE "Simple" algorithm — fixed h_conv
//! values by surface orientation (convection-only, with radiative component
//! already subtracted per ASHRAE 1985 Table 1). Exterior forced convection
//! follows the DOE-2 model with a surface-roughness correction factor.
//!
//! All inputs and outputs are in SI units.
//!
//! # References
//! - EnergyPlus ERM 26.1 — Inside Surface Heat Balance: TARP Algorithm.
//! - EnergyPlus ConvectionCoefficients.cc `CalcASHRAESimpleIntConvCoeff`.
//! - EnergyPlus ERM 26.1 — Outside Surface Heat Balance: DOE-2 Exterior Convection.
//! - OCHRE reference implementation `calculate_film_resistances`.
//! - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, pp 79.
//! - ASHRAE Handbook of Fundamentals 1985, p. 23.2, Table 1.
//! - ASHRAE HoF 2021 Ch. 15, Table 1 (fenestration film coefficients).
//! - Engineers Edge surface heat transfer coefficients (citing ASHRAE
//!   peak-load convention): 34.0 W/(m²·°C) winter, 22.7 W/(m²·°C) summer.

use hares_types::HaresError;

/// Combined exterior film coefficient [W/(m²·K)] — ASHRAE conventional
/// (convective + radiative) peak-load value at ~15 mph (6.7 m/s) wind for
/// opaque outer surfaces. Commonly applied to fenestration in simplified
/// load-calculation contexts as a fallback when explicit film resistance is
/// unavailable.
///
/// This is **not** the NFRC 100 / ISO 15099 convective boundary condition
/// (26 W/(m²·K) at 5.5 m/s wind, formula h_cv = 4 + 4·V). The 34 W/(m²·K)
/// value is the ASHRAE conventional combined coefficient used for peak
/// heating load calculations. See ASHRAE HoF 2021 Ch. 15, Table 1.
///
/// Used in `hares-envelope::thermal_solver::longwave` as the fallback
/// exterior film coefficient for window longwave radiation correction, and
/// in `hares-core::dwelling::solver_builder` as the fallback when a
/// computed exterior film resistance is unavailable.
pub const H_OUT_ASHRAE_PEAK: f64 = 34.0;

/// Zone height ordering for TARP above/below determination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneLabel {
    Ground,      // GND
    Foundation,  // FND
    Conditioned, // LIV
    Garage,      // GAR
    Attic,       // ATC
    Outdoor,     // EXT
}

impl ZoneLabel {
    /// Ordinal height rank (0 = lowest, 5 = highest / outdoor).
    ///
    /// Matches the `zone_order` list in the OCHRE reference implementation.
    pub fn height_order(self) -> u8 {
        match self {
            Self::Ground => 0,
            Self::Foundation => 1,
            Self::Conditioned => 2,
            Self::Garage => 3,
            Self::Attic => 4,
            Self::Outdoor => 5,
        }
    }
}

/// Surface roughness factor for DOE-2 exterior forced convection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRoughness {
    VeryRough,
    Rough,
    MediumRough,
    MediumSmooth,
    Smooth,
    VerySmooth,
}

impl SurfaceRoughness {
    /// DOE-2 roughness correction factor `r_f`.
    ///
    /// Scales the excess convection coefficient `(h_glass - h_natural)`.
    pub fn factor(self) -> f64 {
        match self {
            Self::VeryRough => 2.17,
            Self::Rough => 1.67,
            Self::MediumRough => 1.52,
            Self::MediumSmooth => 1.13,
            Self::Smooth => 1.11,
            Self::VerySmooth => 1.00,
        }
    }
}

/// The HPXML element whose material an outside surface's roughness comes
/// from: `Siding` on a wall or rim joist, `RoofType` on a roof, `Type` on a
/// foundation wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutsideLayer {
    Siding,
    Roof,
    FoundationWall,
}

impl OutsideLayer {
    /// The material OS-HPXML v1.12.0 gives this layer when the HPXML names
    /// none, and where it does so.
    fn os_hpxml_default(self) -> (&'static str, &'static str) {
        match self {
            Self::Siding => ("wood siding", "defaults.rb:1338, :1379"),
            Self::Roof => ("asphalt or fiberglass shingles", "defaults.rb:1268"),
            Self::FoundationWall => ("solid concrete", "defaults.rb:1431-1433"),
        }
    }

    /// The values of this layer's HPXML element that name no material.
    fn names_no_material(self, value: &str) -> bool {
        match self {
            Self::Siding => matches!(value, "none" | "other" | "unknown" | "not present"),
            Self::Roof => matches!(value, "other" | "no one major type" | "cool roof"),
            Self::FoundationWall => false,
        }
    }
}

/// One HPXML outside material and the EnergyPlus record its roughness comes
/// from.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRoughness {
    pub layer: OutsideLayer,
    /// The HPXML enumeration value.
    pub material: &'static str,
    /// The record's name in EnergyPlus v24.2.0's
    /// `datasets/ASHRAE_2005_HOF_Materials.idf`, or the source that stands
    /// in for one.
    pub record: &'static str,
    /// The line of the record's name in that file; 0 where the source is
    /// not a dataset record.
    pub line: u32,
    pub class: SurfaceRoughness,
}

const fn row(
    layer: OutsideLayer,
    material: &'static str,
    record: &'static str,
    line: u32,
    class: SurfaceRoughness,
) -> MaterialRoughness {
    MaterialRoughness {
        layer,
        material,
        record,
        line,
        class,
    }
}

/// Every HPXML outside material and its roughness class.
///
/// One rule: a material takes the roughness of the record that names it in
/// EnergyPlus v24.2.0's `datasets/ASHRAE_2005_HOF_Materials.idf`, searched
/// family by family: the siding records (`Siding:`, lines 2022-2077) for a
/// siding; the shingle records (`Shingles:`, 2002-2017) and then the roofing
/// records (1887-1912) for a roof; then the exterior finish layers (F06-F15,
/// 116-188), the masonry records (M01-M09, 332-396) and the concrete
/// records (1018-1218). Where the material is a kind of a named one, it
/// takes that record: vinyl and aluminum siding are the hollow-backed
/// siding, fiber-cement siding the cement siding, composite shingle siding
/// and generic shingles the asphalt shingles, a single-ply sheeting the
/// built-up roofing (the roofing family's only membrane), concrete tiles
/// the slate or tile layer. A green roof has no record and takes
/// `Material:RoofVegetation`'s default roughness (v24.2.0
/// `idd/Energy+.idd.in`, lines 5064-5072).
///
/// The Engineering Reference's Walton table lists stucco and brick as its
/// Very Rough and Rough examples; the dataset's stucco (F07) is Smooth and
/// its brick (M01) Medium Rough, and the dataset is followed. Every row
/// departs from OS-HPXML v1.12.0, which gives every surface `Rough`
/// (`model.rb:49`, `:95`).
pub const MATERIAL_ROUGHNESS: &[MaterialRoughness] = {
    use OutsideLayer::{FoundationWall as F, Roof as R, Siding as S};
    use SurfaceRoughness::*;
    &[
        row(
            S,
            "wood siding",
            "Siding: Wood - bevel 13 by 200mm - lapped",
            2047,
            Rough,
        ),
        row(S, "stucco", "F07 25mm stucco", 124, Smooth),
        row(S, "synthetic stucco", "F06 EIFS finish", 116, Smooth),
        row(S, "vinyl siding", "Siding: Hollow-backed", 2062, Smooth),
        row(S, "aluminum siding", "Siding: Hollow-backed", 2062, Smooth),
        row(S, "brick veneer", "M01 100mm brick", 332, MediumRough),
        row(S, "stone veneer", "F10 25mm stone", 148, MediumRough),
        row(
            S,
            "asbestos siding",
            "Siding: Asbestos-cement 6.4mm",
            2022,
            VeryRough,
        ),
        row(
            S,
            "fiber cement siding",
            "Siding: Asbestos-cement 6.4mm",
            2022,
            VeryRough,
        ),
        row(
            S,
            "composite shingle siding",
            "Asphalt shingles",
            1897,
            VeryRough,
        ),
        row(
            S,
            "masonite siding",
            "Siding: Hardboard 11mm",
            2037,
            MediumSmooth,
        ),
        row(
            R,
            "asphalt or fiberglass shingles",
            "Asphalt shingles",
            1897,
            VeryRough,
        ),
        row(R, "shingles", "Asphalt shingles", 1897, VeryRough),
        row(
            R,
            "wood shingles or shakes",
            "Shingles: Wood 400mm - 190-mm exposure",
            2007,
            MediumRough,
        ),
        row(R, "slate or tile shingles", "Slate - 13mm", 1907, VeryRough),
        row(R, "concrete tiles", "F14 Slate or tile", 180, VeryRough),
        row(R, "metal surfacing", "F08 Metal surface", 132, Smooth),
        row(
            R,
            "plastic/rubber/synthetic sheeting",
            "Built-up roofing - 10mm",
            1902,
            VeryRough,
        ),
        row(
            R,
            "expanded polystyrene sheathing",
            "Insulation: Expanded polystyrene - molded beads - 16kg/m3 density",
            810,
            VeryRough,
        ),
        row(
            R,
            "concrete",
            "Concrete: Sand and gravel or stone aggregate concretes - 2400 kg/m3 - 51mm",
            1018,
            MediumRough,
        ),
        row(
            R,
            "green roof",
            "Material:RoofVegetation (Energy+.idd.in:5064-5072)",
            0,
            MediumRough,
        ),
        row(
            F,
            "solid concrete",
            "Concrete: Sand and gravel or stone aggregate concretes - 2400 kg/m3 - 51mm",
            1018,
            MediumRough,
        ),
        row(
            F,
            "concrete block",
            "M05 200mm concrete block",
            364,
            MediumRough,
        ),
        row(
            F,
            "concrete block foam core",
            "M05 200mm concrete block",
            364,
            MediumRough,
        ),
        row(
            F,
            "concrete block perlite core",
            "M05 200mm concrete block",
            364,
            MediumRough,
        ),
        row(
            F,
            "concrete block vermiculite core",
            "M05 200mm concrete block",
            364,
            MediumRough,
        ),
        row(
            F,
            "concrete block solid core",
            "M05 200mm concrete block",
            364,
            MediumRough,
        ),
        row(F, "double brick", "M01 100mm brick", 332, MediumRough),
        row(
            F,
            "wood",
            "Siding: Wood - plywood 9.5mm - lapped",
            2057,
            Rough,
        ),
    ]
};

/// The roughness class of an outside surface whose `layer` is `material`
/// ([`MATERIAL_ROUGHNESS`]), with a warning when the HPXML names no
/// material: the layer then takes the material OS-HPXML v1.12.0 defaults
/// an absent element to (wood siding, asphalt shingles, solid concrete).
/// An explicit value that names no material ("other", "unknown", "not
/// present", "none", "no one major type", "cool roof") is treated as
/// absent; OS-HPXML itself defaults only an absent element.
///
/// # Errors
///
/// A `material` that is not a value of the layer's HPXML element.
pub fn outside_layer_roughness(
    layer: OutsideLayer,
    material: Option<&str>,
) -> Result<(SurfaceRoughness, Option<String>), HaresError> {
    let (default, source) = layer.os_hpxml_default();
    let (material, warning) = match material {
        Some(value) if !layer.names_no_material(value) => (value, None),
        given => (
            default,
            Some(match given {
                None => format!(
                    "the outside {layer:?} layer names no material; its roughness is that of \
                     OS-HPXML's default for an absent element, {default} ({source})"
                ),
                Some(value) => format!(
                    "the outside {layer:?} layer's '{value}' names no material; it is treated \
                     as absent and takes the roughness of OS-HPXML's default for an absent \
                     element, {default} ({source})"
                ),
            }),
        ),
    };
    let row = MATERIAL_ROUGHNESS
        .iter()
        .find(|row| row.layer == layer && row.material == material)
        .ok_or_else(|| {
            HaresError::Physics(format!(
                "outside material '{material}' is not an HPXML value for a {layer:?}"
            ))
        })?;
    Ok((row.class, warning))
}

/// Typical zone temperatures [°C] for each [`ZoneLabel`] variant.
///
/// Anchor values: Ground = `avg_ground_c`, Conditioned = 20 °C,
/// Outdoor = `avg_ambient_c + 5`.  Foundation, Garage, and Attic are linearly
/// interpolated between adjacent anchors, matching the behaviour of
/// `pandas.Series.interpolate()` in the OCHRE reference.
///
/// Returns `[ground, foundation, conditioned, garage, attic, outdoor]`.
pub fn typical_zone_temps(avg_ground_c: f64, avg_ambient_c: f64) -> [f64; 6] {
    let t_ground = avg_ground_c;
    let t_conditioned = 20.0_f64;
    let t_outdoor = avg_ambient_c + 5.0;

    // Linear interpolation between the three anchors:
    //   Ground(0) -- Foundation(1) -- Conditioned(2) -- Garage(3) -- Attic(4) -- Outdoor(5)
    let t_foundation = t_ground + (t_conditioned - t_ground) * (1.0 / 2.0);
    let t_garage = t_conditioned + (t_outdoor - t_conditioned) * (1.0 / 3.0);
    let t_attic = t_conditioned + (t_outdoor - t_conditioned) * (2.0 / 3.0);

    [
        t_ground,
        t_foundation,
        t_conditioned,
        t_garage,
        t_attic,
        t_outdoor,
    ]
}

/// TARP natural convection coefficient h [W/(m²·K)].
///
/// - Vertical surface (`tilt_deg == 90`): `h = 1.31 · ΔT^(1/3)`
/// - Non-vertical, warm side above (`above_hotter = true`):
///   `h = 9.482 · ΔT^(1/3) / (7.238 − |cos(tilt)|)` (enhanced)
/// - Non-vertical, warm side below (`above_hotter = false`):
///   `h = 1.810 · ΔT^(1/3) / (1.382 + |cos(tilt)|)` (reduced)
///
/// `delta_t_k` must be non-negative; `tilt_deg` is measured from horizontal.
pub fn tarp_h_natural(tilt_deg: f64, delta_t_k: f64, above_hotter: bool) -> f64 {
    let cbrt_dt = delta_t_k.cbrt();
    if (tilt_deg - 90.0).abs() < 1e-9 {
        1.31 * cbrt_dt
    } else {
        let cos_tilt = tilt_deg.to_radians().cos().abs();
        if above_hotter {
            9.482 * cbrt_dt / (7.238 - cos_tilt)
        } else {
            1.810 * cbrt_dt / (1.382 + cos_tilt)
        }
    }
}

/// ASHRAE "Simple" interior convection coefficient h_conv [W/(m²·K)].
///
/// Fixed convection-only values by surface orientation, derived from
/// ASHRAE 1985 Table 1 surface conductances (ε = 0.9) with the radiative
/// component subtracted.  These are the default interior convection
/// coefficients in EnergyPlus (`CalcASHRAESimpleIntConvCoeff`).
///
/// For vertical surfaces, returns the fixed value 3.076 regardless of ΔT.
/// For non-vertical surfaces, the buoyancy direction (enhanced vs reduced)
/// depends on which side is warmer and the surface tilt — this is computed
/// from the zone temperatures and the `above_hotter` flag.
///
/// | Orientation             | Condition      | h_conv [W/(m²·K)] |
/// |-------------------------|----------------|--------------------|
/// | Vertical (67.5–112.5°)  | —              | 3.076              |
/// | Horizontal, enhanced    | heat flow up   | 4.040              |
/// | Horizontal, reduced     | heat flow down | 0.948              |
/// | Tilted, enhanced        | heat flow up   | 3.870              |
/// | Tilted, reduced         | heat flow down | 2.281              |
///
/// # References
/// - EnergyPlus ConvectionCoefficients.cc:1829-1885.
/// - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, p 79.
/// - ASHRAE Handbook of Fundamentals 1985, p. 23.2, Table 1.
pub fn ashrae_simple_interior_h_conv(
    tilt_deg: f64,
    _t_ext_c: f64,
    _t_int_c: f64,
    above_hotter: bool,
) -> f64 {
    let cos_tilt = tilt_deg.to_radians().cos().abs();
    if cos_tilt < 0.3827 {
        3.076
    } else if cos_tilt >= 0.9239 {
        if above_hotter { 4.040 } else { 0.948 }
    } else if above_hotter {
        3.870
    } else {
        2.281
    }
}

/// Film resistances for a building envelope boundary [m²·K/W].
///
/// Returns `(r_interior, r_exterior)`.
///
/// Interior resistance uses TARP natural convection.  Exterior resistance
/// additionally applies the DOE-2 forced-convection model when
/// `exterior_zone` is [`ZoneLabel::Outdoor`]; otherwise it equals the
/// interior resistance.
///
/// # Parameters
/// - `tilt_deg` -- surface tilt in degrees from horizontal (0 = floor/ceiling,
///   90 = vertical wall).
/// - `interior_zone` / `exterior_zone` -- zones bounding the surface.
/// - `avg_wind_speed_m_s` -- site average wind speed [m/s].
/// - `avg_ground_temp_c` -- annual average ground temperature [°C].
/// - `avg_ambient_temp_c` -- annual average outdoor dry-bulb temperature [°C].
/// - `roughness` -- surface roughness class for the DOE-2 `r_f` factor.
pub fn film_resistances(
    tilt_deg: f64,
    interior_zone: ZoneLabel,
    exterior_zone: ZoneLabel,
    avg_wind_speed_m_s: f64,
    avg_ground_temp_c: f64,
    avg_ambient_temp_c: f64,
    roughness: SurfaceRoughness,
) -> (f64, f64) {
    let temps = typical_zone_temps(avg_ground_temp_c, avg_ambient_temp_c);
    let t_int = temps[interior_zone.height_order() as usize];
    let t_ext = temps[exterior_zone.height_order() as usize];

    let ext_above = exterior_zone.height_order() > interior_zone.height_order();
    let t_ext_hotter = t_ext >= t_int;
    let above_hotter = !(ext_above ^ t_ext_hotter);

    // ASHRAE "Simple" interior convection algorithm — fixed h_conv [W/(m²·K)]
    // by surface orientation, with the radiative component already subtracted.
    //
    // EnergyPlus ConvectionCoefficients.cc CalcASHRAESimpleIntConvCoeff returns
    // these convection-only values derived from ASHRAE 1985 Table 1 surface
    // conductances at ε = 0.9, minus the radiative component
    // (1.02 × 0.9 = 0.918 BTU/h·ft²·°F), converted to SI.  These are the
    // default interior convection coefficients in E+ and match OCHRE's
    // TARP-at-12.9°C-floor result (envelope.py:374) to within 0.12%.
    //
    // For a vertical surface: h_conv = 3.076 W/(m²·K).  Combined with
    // h_rad ≈ 5.14 at ε = 0.9, T_ref = 293.15 K, the total h_si = 8.22
    // is consistent with ASHRAE 140-2017 Table 25 (h_si = 8.29).
    //
    // References:
    // - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, p 79.
    // - ASHRAE Handbook of Fundamentals 1985, p. 23.2, Table 1.
    // - EnergyPlus ConvectionCoefficients.cc:1829-1885.
    // - ASHRAE 140-2017 §5.3.1.9, Table 25.
    let h_conv = ashrae_simple_interior_h_conv(tilt_deg, t_ext, t_int, above_hotter);

    let r_int = 1.0 / h_conv;

    let r_ext = if exterior_zone == ZoneLabel::Outdoor {
        let ext_above = exterior_zone.height_order() > interior_zone.height_order();
        let t_ext_hotter = t_ext >= t_int;
        let above_hotter = !(ext_above ^ t_ext_hotter);
        let delta_t = (t_ext - t_int).abs().max(0.1);
        let h_natural = tarp_h_natural(tilt_deg, delta_t, above_hotter);
        let h_glass = (h_natural.powi(2) + (3.40 * avg_wind_speed_m_s.powf(0.75)).powi(2)).sqrt();
        let h_forced = roughness.factor() * (h_glass - h_natural);
        // Floor the exterior convection coefficient to 1.0 W/(m²·K) — the
        // natural convection floor for a vertical surface at ΔT ≈ 0.4°C
        // (ASHRAE HoF 2021 Ch. 4 §4.2). At zero wind and ΔT → 0, the DOE-2
        // model produces h_natural + h_forced → 0; dividing by near-zero
        // produces numerically unstable film resistance that causes the window
        // exterior LWR T_eff correction to diverge (h_out appears in the
        // denominator at longwave.rs:106). A minimum of 1.0 protects all
        // consumers of exterior film resistance, not just the window path.
        let h_ext = (h_natural + h_forced).max(1.0);
        1.0 / h_ext
    } else if exterior_zone == ZoneLabel::Ground {
        1.0 / h_conv
    } else {
        r_int
    };

    (r_int, r_ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_utils::approx_eq;

    fn assert_approx(actual: f64, expected: f64, tol: f64) {
        approx_eq(actual, expected, tol);
    }

    #[test]
    fn vertical_surface_natural_h_and_resistance() {
        // h = 1.31 * 12.9^(1/3) ≈ 3.0759; R = 1/h ≈ 0.3251
        let h = tarp_h_natural(90.0, 12.9, true);
        let expected_h = 1.31 * 12.9_f64.cbrt();
        assert_approx(h, expected_h, 1e-10);
        assert_approx(1.0 / h, 1.0 / expected_h, 1e-6);
    }

    #[test]
    fn enhanced_tilt_above_hotter() {
        // tilt=0 (horizontal), above_hotter=true → enhanced formula
        // h = 9.482 * 12.9^(1/3) / (7.238 - |cos(0 rad)|)
        //   = 9.482 * 12.9^(1/3) / (7.238 - 1.0)
        let h = tarp_h_natural(0.0, 12.9, true);
        let expected_h = 9.482 * 12.9_f64.cbrt() / (7.238 - 0.0_f64.to_radians().cos().abs());
        assert_approx(h, expected_h, 1e-10);
    }

    #[test]
    fn film_resistances_typical_wall_outdoor() {
        // Interior: ASHRAE Simple gives h_conv = 3.076 for vertical surface.
        // Exterior: TARP natural at ΔT = 5.0°C + DOE-2 forced at 2 m/s.
        let (r_int, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        let h_conv = 3.076_f64;
        let h_rad = crate::constants::linearised_h_rad(0.9, 293.15);
        assert_approx(r_int, 1.0 / h_conv, 1e-10);
        assert!(
            r_int > 1.0 / (h_conv + h_rad),
            "conv-only r_int={r_int} must be > combined {:.4}",
            1.0 / (h_conv + h_rad)
        );
        let delta_t = 5.0_f64;
        let h_natural = 1.31 * delta_t.cbrt();
        let h_glass = (h_natural.powi(2) + (3.40 * 2.0_f64.powf(0.75)).powi(2)).sqrt();
        let h_forced = 1.67 * (h_glass - h_natural);
        assert_approx(r_ext, 1.0 / (h_natural + h_forced), 1e-6);
        assert!(r_ext < r_int, "r_ext={r_ext} should be < r_int={r_int}");
    }

    #[test]
    fn film_resistances_interior_boundary_no_forced_convection() {
        // LIV interior, ATC exterior -- not outdoor, so r_ext == r_int.
        let (r_int, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Attic,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        assert_approx(r_int, r_ext, 1e-15);
    }

    #[test]
    fn interior_film_resistance_is_convection_only() {
        // ASHRAE Simple gives h_conv = 3.076 for vertical surface,
        // matching E+ CalcASHRAESimpleIntConvCoeff.
        // Linearized h_rad = 4·ε·σ·T_ref³ ≈ 5.14 W/(m²·K) at ε=0.9, T=20°C.
        // In Option 2 (EnergyPlus/TRNSYS/ESP-r), R_film = 1/h_conv (conv-only).
        // h_rad is carried by the star-mesh radiation conductances in the
        // A-matrix, not by R_film.
        let (r_int, _) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        let h_conv = 3.076_f64;
        let h_rad = crate::constants::linearised_h_rad(0.9, 293.15);
        let r_conv_only = 1.0 / h_conv;
        let r_combined = 1.0 / (h_conv + h_rad);

        assert_approx(r_int, r_conv_only, 1e-10);
        assert!(
            r_int > r_combined + 0.1,
            "conv-only R_film ({r_int:.4}) must be significantly larger than combined ({r_combined:.4})"
        );
        assert!(
            (r_int - 0.325).abs() < 0.01,
            "vertical wall conv-only R_film ≈ 0.325, got {r_int:.4}"
        );
    }

    #[test]
    fn exterior_film_resistance_is_convection_only_no_h_rad() {
        // Exterior film resistance for outdoor-facing surfaces uses
        // TARP natural + DOE-2 forced (no h_rad). Exterior LWR is
        // handled by the explicit exterior longwave solver.
        let (_, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        let delta_t = 5.0_f64;
        let h_natural = 1.31 * delta_t.cbrt();
        let h_glass = (h_natural.powi(2) + (3.40 * 2.0_f64.powf(0.75)).powi(2)).sqrt();
        let h_forced = 1.67 * (h_glass - h_natural);
        let r_expected = 1.0 / (h_natural + h_forced);
        assert_approx(r_ext, r_expected, 1e-6);
    }

    #[test]
    fn ashrae_simple_vertical_returns_3_076() {
        assert_approx(
            ashrae_simple_interior_h_conv(90.0, 15.0, 20.0, true),
            3.076,
            1e-10,
        );
    }

    #[test]
    fn ashrae_simple_horizontal_enhanced() {
        let h = ashrae_simple_interior_h_conv(0.0, 15.0, 20.0, true);
        assert_approx(h, 4.040, 1e-10);
    }

    #[test]
    fn ashrae_simple_horizontal_reduced() {
        let h = ashrae_simple_interior_h_conv(0.0, 25.0, 20.0, false);
        assert_approx(h, 0.948, 1e-10);
    }

    /// When delta_t=0, all TARP formulas produce h=0 (cbrt(0)=0).
    /// This is physically correct: no temperature difference means no
    /// buoyancy-driven convection.
    #[test]
    fn tarp_h_natural_zero_delta_t_returns_zero() {
        let h_vert = tarp_h_natural(90.0, 0.0, true);
        assert_approx(h_vert, 0.0, 1e-15);

        let h_horiz_above = tarp_h_natural(0.0, 0.0, true);
        assert_approx(h_horiz_above, 0.0, 1e-15);

        let h_horiz_below = tarp_h_natural(0.0, 0.0, false);
        assert_approx(h_horiz_below, 0.0, 1e-15);
    }

    #[test]
    fn typical_zone_temps_known_anchors_and_interpolation() {
        let temps = typical_zone_temps(10.0, 10.0);
        // Anchors
        assert_approx(
            temps[ZoneLabel::Ground.height_order() as usize],
            10.0,
            1e-12,
        );
        assert_approx(
            temps[ZoneLabel::Conditioned.height_order() as usize],
            20.0,
            1e-12,
        );
        assert_approx(
            temps[ZoneLabel::Outdoor.height_order() as usize],
            15.0,
            1e-12,
        );
        // Interpolated: Foundation = 10 + (20-10)*(1/2) = 15
        assert_approx(
            temps[ZoneLabel::Foundation.height_order() as usize],
            15.0,
            1e-12,
        );
        // Garage = 20 + (15-20)*(1/3) = 20 - 5/3 ≈ 18.333...
        assert_approx(
            temps[ZoneLabel::Garage.height_order() as usize],
            20.0 + (15.0 - 20.0) / 3.0,
            1e-12,
        );
        // Attic = 20 + (15-20)*(2/3) = 20 - 10/3 ≈ 16.666...
        assert_approx(
            temps[ZoneLabel::Attic.height_order() as usize],
            20.0 + (15.0 - 20.0) * 2.0 / 3.0,
            1e-12,
        );
    }

    // --- outside material roughness tests ---

    /// Every HPXML siding, roof and foundation wall material takes the
    /// roughness of its EnergyPlus v24.2.0 `ASHRAE_2005_HOF_Materials.idf`
    /// record, transcribed here from the dataset (record name, line of the
    /// name, its Roughness field). Each HPXML enumeration value is listed
    /// once, so a material the table drops, or a record it changes, fails.
    #[test]
    fn every_hpxml_material_takes_its_energyplus_roughness() {
        use OutsideLayer::{FoundationWall as F, Roof as R, Siding as S};
        use SurfaceRoughness::*;
        let expected: &[(OutsideLayer, &str, &str, u32, SurfaceRoughness)] = &[
            (
                S,
                "wood siding",
                "Siding: Wood - bevel 13 by 200mm - lapped",
                2047,
                Rough,
            ),
            (S, "stucco", "F07 25mm stucco", 124, Smooth),
            (S, "synthetic stucco", "F06 EIFS finish", 116, Smooth),
            (S, "vinyl siding", "Siding: Hollow-backed", 2062, Smooth),
            (S, "aluminum siding", "Siding: Hollow-backed", 2062, Smooth),
            (S, "brick veneer", "M01 100mm brick", 332, MediumRough),
            (S, "stone veneer", "F10 25mm stone", 148, MediumRough),
            (
                S,
                "asbestos siding",
                "Siding: Asbestos-cement 6.4mm",
                2022,
                VeryRough,
            ),
            (
                S,
                "fiber cement siding",
                "Siding: Asbestos-cement 6.4mm",
                2022,
                VeryRough,
            ),
            (
                S,
                "composite shingle siding",
                "Asphalt shingles",
                1897,
                VeryRough,
            ),
            (
                S,
                "masonite siding",
                "Siding: Hardboard 11mm",
                2037,
                MediumSmooth,
            ),
            (
                R,
                "asphalt or fiberglass shingles",
                "Asphalt shingles",
                1897,
                VeryRough,
            ),
            (R, "shingles", "Asphalt shingles", 1897, VeryRough),
            (
                R,
                "wood shingles or shakes",
                "Shingles: Wood 400mm - 190-mm exposure",
                2007,
                MediumRough,
            ),
            (R, "slate or tile shingles", "Slate - 13mm", 1907, VeryRough),
            (R, "concrete tiles", "F14 Slate or tile", 180, VeryRough),
            (R, "metal surfacing", "F08 Metal surface", 132, Smooth),
            (
                R,
                "plastic/rubber/synthetic sheeting",
                "Built-up roofing - 10mm",
                1902,
                VeryRough,
            ),
            (
                R,
                "expanded polystyrene sheathing",
                "Insulation: Expanded polystyrene - molded beads - 16kg/m3 density",
                810,
                VeryRough,
            ),
            (
                R,
                "concrete",
                "Concrete: Sand and gravel or stone aggregate concretes - 2400 kg/m3 - 51mm",
                1018,
                MediumRough,
            ),
            (
                R,
                "green roof",
                "Material:RoofVegetation (Energy+.idd.in:5064-5072)",
                0,
                MediumRough,
            ),
            (
                F,
                "solid concrete",
                "Concrete: Sand and gravel or stone aggregate concretes - 2400 kg/m3 - 51mm",
                1018,
                MediumRough,
            ),
            (
                F,
                "concrete block",
                "M05 200mm concrete block",
                364,
                MediumRough,
            ),
            (
                F,
                "concrete block foam core",
                "M05 200mm concrete block",
                364,
                MediumRough,
            ),
            (
                F,
                "concrete block perlite core",
                "M05 200mm concrete block",
                364,
                MediumRough,
            ),
            (
                F,
                "concrete block vermiculite core",
                "M05 200mm concrete block",
                364,
                MediumRough,
            ),
            (
                F,
                "concrete block solid core",
                "M05 200mm concrete block",
                364,
                MediumRough,
            ),
            (F, "double brick", "M01 100mm brick", 332, MediumRough),
            (
                F,
                "wood",
                "Siding: Wood - plywood 9.5mm - lapped",
                2057,
                Rough,
            ),
        ];
        assert_eq!(
            MATERIAL_ROUGHNESS.len(),
            expected.len(),
            "one row per material"
        );
        for &(layer, material, record, line, class) in expected {
            let row = MATERIAL_ROUGHNESS
                .iter()
                .find(|row| row.layer == layer && row.material == material)
                .unwrap_or_else(|| panic!("{layer:?} '{material}' has no row"));
            assert_eq!((row.record, row.line), (record, line), "{material}");
            let (got, warning) = outside_layer_roughness(layer, Some(material)).unwrap();
            assert_eq!(got, class, "{material}");
            assert!(warning.is_none(), "{material} names its material");
        }
    }

    /// A layer that names no material takes the material OS-HPXML defaults
    /// it to, with a warning: wood siding (Rough), asphalt shingles (Very
    /// Rough), solid concrete (Medium Rough). Not a blanket Rough.
    #[test]
    fn a_layer_without_a_material_takes_os_hpxml_default_material() {
        use OutsideLayer::{FoundationWall, Roof, Siding};
        for (layer, given, class, default) in [
            (Siding, None, SurfaceRoughness::Rough, "wood siding"),
            (
                Siding,
                Some("not present"),
                SurfaceRoughness::Rough,
                "wood siding",
            ),
            (
                Siding,
                Some("other"),
                SurfaceRoughness::Rough,
                "wood siding",
            ),
            (Roof, None, SurfaceRoughness::VeryRough, "asphalt"),
            (
                Roof,
                Some("cool roof"),
                SurfaceRoughness::VeryRough,
                "asphalt",
            ),
            (
                Roof,
                Some("no one major type"),
                SurfaceRoughness::VeryRough,
                "asphalt",
            ),
            (
                FoundationWall,
                None,
                SurfaceRoughness::MediumRough,
                "solid concrete",
            ),
        ] {
            let (got, warning) = outside_layer_roughness(layer, given).unwrap();
            assert_eq!(got, class, "{layer:?} {given:?}");
            let warning = warning.expect("a defaulted material is a warning");
            assert!(warning.contains(default), "{warning}");
        }
    }

    /// A value outside a layer's HPXML enumeration is an error, including a
    /// material of another layer.
    #[test]
    fn a_material_outside_the_layer_enumeration_is_an_error() {
        for (layer, material) in [
            (OutsideLayer::Siding, "diamond plate"),
            (OutsideLayer::Siding, "metal surfacing"),
            (OutsideLayer::Roof, "wood siding"),
            (OutsideLayer::FoundationWall, "stucco"),
        ] {
            let err = outside_layer_roughness(layer, Some(material)).unwrap_err();
            assert!(err.to_string().contains(material), "{err}");
        }
    }

    #[test]
    fn smooth_gives_higher_r_ext_than_rough_at_same_wind() {
        // Smooth (Rf=1.11) produces less forced convection, so a higher
        // exterior R, than Rough (Rf=1.67).
        let (_, r_smooth) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            4.0,
            10.0,
            10.0,
            SurfaceRoughness::Smooth,
        );
        let (_, r_rough) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            4.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        assert!(
            r_smooth > r_rough,
            "Smooth R_ext={r_smooth:.6} must exceed Rough R_ext={r_rough:.6}"
        );
    }

    #[test]
    fn very_rough_gives_lower_r_ext_than_rough_at_same_wind() {
        // VeryRough (Rf=2.17) produces more forced convection, so a lower
        // exterior R, than Rough (Rf=1.67).
        let (_, r_very_rough) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            4.0,
            10.0,
            10.0,
            SurfaceRoughness::VeryRough,
        );
        let (_, r_rough) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            4.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        assert!(
            r_very_rough < r_rough,
            "VeryRough R_ext={r_very_rough:.6} must be less than Rough R_ext={r_rough:.6}"
        );
    }

    /// Integration: at minimum delta_t (0.1 °C, clamped from zero) and zero wind
    /// speed, the DOE-2 exterior model produces h_natural + h_forced ≈ 0.608
    /// W/(m²·K). The 1.0 W/(m²·K) floor ensures r_ext = 1.0 / 1.0 = 1.0
    /// rather than 1.0 / 0.608 ≈ 1.645 (or worse, 1.0 / 0 → ∞).
    ///
    /// Without the floor, near-zero exterior convection coefficients produce
    /// numerically unstable film resistance that flows into the window exterior
    /// LWR T_eff correction (h_out in the denominator at longwave.rs:106),
    /// yielding unphysical correction factors.
    #[test]
    fn doe2_exterior_floor_at_zero_wind_min_delta_t() {
        use super::*;
        use crate::test_utils::approx_eq;

        // avg_ambient_c = 15.0 → t_outdoor = 20.0 = t_conditioned → delta_t = 0.1 (clamped)
        let (r_int, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            0.0,  // zero wind speed
            10.0, // avg_ground (irrelevant — not Outdoor)
            15.0, // avg_ambient → t_outdoor = 20.0
            SurfaceRoughness::Smooth,
        );

        // At ΔT = 0.1 °C, vertical surface: h_natural = 1.31 × 0.1^(1/3) ≈ 0.608
        // At zero wind: h_glass = sqrt(h_natural² + 0) = h_natural, forced = 0
        // h_natural + h_forced ≈ 0.608 < 1.0 → floor to 1.0 → r_ext = 1.0
        let expected_r_ext = 1.0;
        approx_eq(r_ext, expected_r_ext, 1e-10);

        // Interior should not be affected — h_conv for vertical ≈ 3.076 → r_int ≈ 0.325
        let expected_r_int = 1.0 / 3.076;
        approx_eq(r_int, expected_r_int, 0.001);

        // Without the floor, r_ext would be ≈ 1.645 (1 / 0.608); our result is smaller.
        assert!(
            r_ext < 1.1,
            "with floor, r_ext={r_ext:.6} should be ≤ 1.0; without floor it would be ~1.645"
        );

        // Now verify at typical wind (2 m/s) the floor does NOT override valid values.
        let (_, r_ext_typical) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        // At 2 m/s, ΔT = 5.0 (t_outdoor = 15.0), h_natural ≈ 2.24, total > 1.0
        // r_ext should be much less than 1.0 (typical exterior R at moderate wind)
        assert!(
            r_ext_typical < 0.5,
            "at 2 m/s wind, r_ext={r_ext_typical:.6} should be well below 1.0 — \
             floor must not override valid values"
        );
    }
}
