//! Envelope LUT parser for loading OCHRE's pre-computed RC values from CSV files.
//!
//! Parses CSV files from the `defaults/envelope/` directory:
//! - `Envelope Boundaries.csv` -- zone label mappings for `(BoundaryType, ZoneType, ZoneType) → boundary name`
//! - `Envelope Boundary Types.csv` -- construction variants with assembly R-values
//! - `Envelope Materials.csv` -- per-layer resistance and capacitance values
//!
//! If `Envelope Boundaries.csv` is absent, boundary name resolution falls back to
//! a built-in hardcoded mapping (see `resolve_boundary_name`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::hpxml::building::{BoundaryType, ZoneType};

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum EnvelopeLutError {
    #[error("CSV parse error in {path}: {reason}")]
    CsvParse { path: PathBuf, reason: String },
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("missing envelope CSV: {0}")]
    MissingFile(PathBuf),
}

// ── Public types ────────────────────────────────────────────────────────────

/// Pre-computed RC layer from OCHRE's material database.
#[derive(Debug, Clone)]
pub struct PrecomputedLayer {
    pub resistance_m2_k_w: f64,
    pub capacitance_kj_m2_k: f64,
}

/// Result of an envelope LUT lookup.
#[derive(Debug, Clone)]
pub struct EnvelopeLookupResult {
    pub layers: Vec<PrecomputedLayer>,
    pub matched_boundary_type: String,
    pub matched_r_value: f64,
}

// ── CSV row types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct BoundaryTypeRow {
    #[serde(rename = "Boundary Name")]
    boundary_name: String,
    #[serde(rename = "Boundary Type")]
    boundary_type: String,
    #[serde(rename = "Finish Type")]
    finish_type: String,
    #[serde(rename = "Construction Type")]
    construction_type: String,
    #[serde(rename = "Insulation Details")]
    insulation_details: String,
    #[serde(rename = "Assembly R Value")]
    assembly_r_value: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct MaterialRow {
    #[serde(rename = "Boundary Name")]
    boundary_name: String,
    #[serde(rename = "Boundary Type")]
    boundary_type: String,
    #[serde(rename = "Resistance (m^2-K/W)")]
    resistance: f64,
    #[serde(rename = "Capacitance (kJ/m^2-K)")]
    capacitance: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct BoundaryRow {
    #[serde(rename = "Boundary Name")]
    boundary_name: String,
    #[serde(rename = "Boundary Label")]
    #[allow(dead_code)]
    // Why: field must exist for serde to match the CSV column header;
    // the label is not used in the Rust lookup but is part of the OCHRE schema.
    boundary_label: String,
    #[serde(rename = "Exterior Zone Label")]
    exterior_zone_label: String,
    #[serde(rename = "Interior Zone Label")]
    interior_zone_label: String,
}

/// Map OCHRE short zone label to HARES `ZoneType`.
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

/// Map an OCHRE boundary name to the associated HPXML `BoundaryType`.
///
/// OCHRE defines boundary names that are more specific than HPXML's
/// generic `Wall`/`Roof`/`Floor` types. This function classifies an
/// OCHRE name into its HPXML type for the CSV-derived lookup table.
fn boundary_type_from_boundary_name(name: &str) -> Option<BoundaryType> {
    match name {
        // Wall boundaries
        "Exterior Wall"
        | "Interior Wall"
        | "Attic Wall"
        | "Garage Wall"
        | "Garage Attached Wall"
        | "Adjacent Wall"
        | "Adjacent Attic Wall"
        | "Adjacent Garage Wall" => Some(BoundaryType::Wall),
        // Roof boundaries
        "Roof" | "Attic Roof" | "Garage Roof" | "Adjacent Ceiling" => Some(BoundaryType::Roof),
        // Floor boundaries (horizontal, exposed to unconditioned spaces)
        "Raised Floor"
        | "Attic Floor"
        | "Foundation Ceiling"
        | "Garage Interior Ceiling"
        | "Garage Ceiling"
        | "Adjacent Floor" => Some(BoundaryType::Floor),
        // Slab boundaries (on-grade, ground contact)
        "Floor" | "Foundation Floor" | "Garage Floor" => Some(BoundaryType::Slab),
        "Window" => Some(BoundaryType::Window),
        "Door" | "Garage Door" => Some(BoundaryType::Door),
        "Foundation Wall" | "Adjacent Foundation Wall" => Some(BoundaryType::FoundationWall),
        "Rim Joist" | "Adjacent Rim Joist" => Some(BoundaryType::RimJoist),
        // Same-zone furniture entries — not real boundary types
        "Indoor Furniture" | "Foundation Furniture" | "Attic Furniture" | "Garage Furniture" => {
            None
        }
        _ => None,
    }
}

// ── Film R-value constants ──────────────────────────────────────────────────

/// Boundary names that use the higher film R-value (floors/ceilings).
const FLOOR_CEILING_BOUNDARIES: &[&str] = &[
    "Attic Floor",
    "Garage Interior Ceiling",
    "Garage Ceiling",
    "Foundation Ceiling",
    "Adjacent Ceiling",
    "Adjacent Floor",
];

/// Film R-value for floor/ceiling boundaries [m²·K/W].
const FILM_R_FLOOR_CEILING: f64 = 0.2642;
/// Film R-value for wall/roof boundaries [m²·K/W].
const FILM_R_WALL_ROOF: f64 = 0.1585;

// ── EnvelopeLookup ──────────────────────────────────────────────────────────

/// Split a variant label into (prefix, insulation_rank).
///
/// "Ceiling R-13" → Some(("Ceiling ", 13))
/// "WoodStud, aluminum siding, R-11" → Some(("WoodStud, aluminum siding, ", 11))
/// "Uninsulated" → Some(("", 0))
/// "Minimal" or unrecognised → None
///
/// This allows monotonicity checks to group variants that differ only in
/// insulation level, not in construction type or siding material.
fn split_r_variant(variant: &str) -> Option<(&str, u32)> {
    if variant.eq_ignore_ascii_case("Uninsulated") {
        return Some(("", 0));
    }
    // Find "R-<digits>" pattern. Look for "R-" followed by at least one digit.
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

/// Loaded and indexed envelope lookup tables.
#[derive(Debug, Clone)]
pub struct EnvelopeLookup {
    /// Boundary type rows grouped by boundary name.
    boundary_types: HashMap<String, Vec<BoundaryTypeRow>>,
    /// Material rows grouped by (boundary_name, boundary_type).
    materials: HashMap<(String, String), Vec<MaterialRow>>,
    /// CSV-derived boundary name map: `(BoundaryType, interior_zone, exterior_zone) → boundary_name`.
    /// Populated from `Envelope Boundaries.csv`. Empty when the file is absent.
    boundary_names: HashMap<(BoundaryType, ZoneType, ZoneType), String>,
}

impl EnvelopeLookup {
    /// Parse the envelope CSV files from `dir`.
    ///
    /// `Envelope Boundary Types.csv` and `Envelope Materials.csv` are required.
    /// `Envelope Boundaries.csv` is optional — when absent, boundary name
    /// resolution falls back to the built-in hardcoded mapping.
    pub fn load(dir: &Path) -> Result<Self, EnvelopeLutError> {
        let bt_path = dir.join("Envelope Boundary Types.csv");
        let mat_path = dir.join("Envelope Materials.csv");
        let boundaries_path = dir.join("Envelope Boundaries.csv");

        if !bt_path.exists() {
            return Err(EnvelopeLutError::MissingFile(bt_path));
        }
        if !mat_path.exists() {
            return Err(EnvelopeLutError::MissingFile(mat_path));
        }

        let boundary_types = Self::load_boundary_types(&bt_path)?;
        let materials = Self::load_materials(&mat_path)?;
        let boundary_names = if boundaries_path.exists() {
            let names = Self::load_boundaries(&boundaries_path)?;
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                let check_lut = Self {
                    boundary_types: boundary_types.clone(),
                    materials: materials.clone(),
                    boundary_names: names.clone(),
                };
                check_lut.check_csv_vs_hardcoded_consistency();
                check_lut.check_material_invariants(&mat_path);
            }
            names
        } else {
            tracing::warn!(
                path = %boundaries_path.display(),
                "Envelope Boundaries.csv not found; falling back to hardcoded boundary name mapping"
            );
            HashMap::new()
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if boundary_names.is_empty() {
            let check_lut = Self {
                boundary_types: boundary_types.clone(),
                materials: materials.clone(),
                boundary_names: HashMap::new(),
            };
            check_lut.check_material_invariants(&mat_path);
        }

        Ok(Self {
            boundary_types,
            materials,
            boundary_names,
        })
    }

    /// Resolve a boundary name from the CSV-derived lookup table.
    ///
    /// When `Envelope Boundaries.csv` was loaded, this returns the boundary
    /// name for the given `(BoundaryType, ZoneType, ZoneType)` triple.
    /// Falls back to the hardcoded `resolve_boundary_name()` when the CSV
    /// does not contain a matching entry or was not loaded.
    pub fn resolve_name(
        &self,
        boundary_type: &BoundaryType,
        interior_zone: Option<&ZoneType>,
        exterior_zone: Option<&ZoneType>,
    ) -> Option<&str> {
        let default_int = ZoneType::Conditioned;
        let default_ext = match boundary_type {
            BoundaryType::Roof
            | BoundaryType::Wall
            | BoundaryType::Door
            | BoundaryType::RimJoist => ZoneType::Outdoor,
            BoundaryType::Slab | BoundaryType::FoundationWall => ZoneType::Ground,
            BoundaryType::Floor => ZoneType::Attic,
            _ => ZoneType::Outdoor,
        };
        let int = interior_zone.unwrap_or(&default_int);
        let ext = exterior_zone.unwrap_or(&default_ext);

        // Same-zone → furniture (thermal mass). These are not in the CSV's
        // boundary_name map because they don't correspond to a single boundary type.
        if int == ext {
            return match int {
                ZoneType::Conditioned => Some("Indoor Furniture"),
                ZoneType::Foundation => Some("Foundation Furniture"),
                ZoneType::Garage => Some("Garage Furniture"),
                _ => None,
            };
        }

        // Adjacent (adiabatic) boundaries — multifamily party walls/floors.
        if *int == ZoneType::Adjacent || *ext == ZoneType::Adjacent {
            let other = if *int == ZoneType::Adjacent { ext } else { int };
            return match (boundary_type, other) {
                (BoundaryType::Wall, ZoneType::Attic) => Some("Adjacent Attic Wall"),
                (BoundaryType::Wall, ZoneType::Garage) => Some("Adjacent Wall"),
                (BoundaryType::Wall, _) => Some("Adjacent Wall"),
                (BoundaryType::Floor, _) => Some("Adjacent Floor"),
                (BoundaryType::Roof, _) => Some("Adjacent Ceiling"),
                (BoundaryType::FoundationWall, _) => Some("Adjacent Foundation Wall"),
                (BoundaryType::RimJoist, _) => Some("Adjacent Rim Joist"),
                _ => None,
            };
        }

        // Try CSV-derived lookup first (the single source of truth when available).
        if !self.boundary_names.is_empty() {
            let key = (boundary_type.clone(), int.clone(), ext.clone());
            if let Some(name) = self.boundary_names.get(&key) {
                return Some(name.as_str());
            }
        }

        // Fall back to hardcoded mapping.
        resolve_boundary_name(boundary_type, Some(int), Some(ext))
    }

    /// Verify CSV-derived boundary names match the hardcoded mapping.
    ///
    /// Logs a warning for each `(BoundaryType, ZoneType, ZoneType)` triple
    /// where the CSV name and the hardcoded name disagree. Activation is
    /// gated on `debug_assertions` or `check_invariants` to avoid runtime
    /// cost in release builds.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn check_csv_vs_hardcoded_consistency(&self) {
        for ((bt, int, ext), csv_name) in &self.boundary_names {
            if let Some(hardcoded_name) = resolve_boundary_name(bt, Some(int), Some(ext)) {
                if csv_name != hardcoded_name {
                    tracing::warn!(
                        boundary_type = ?bt,
                        interior_zone = ?int,
                        exterior_zone = ?ext,
                        csv_name = %csv_name,
                        hardcoded_name = %hardcoded_name,
                        "Boundary name mismatch between CSV and hardcoded mapping"
                    );
                }
            }
        }
    }

    /// Verify STUD AND CAVITY R-values are monotonic within each boundary-name
    /// group and flag layers whose conductivity exceeds a physically plausible
    /// threshold for insulated cavities.
    ///
    /// Activation is gated on `debug_assertions` or `check_invariants`.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn check_material_invariants(&self, mat_path: &Path) {
        let mut rdr = match csv::Reader::from_path(mat_path) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(path = %mat_path.display(), error = %e, "cannot open materials CSV for invariant check");
                return;
            }
        };
        let headers = match rdr.headers() {
            Ok(h) => h.clone(),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read materials CSV headers");
                return;
            }
        };

        let boundary_name_idx = match headers.iter().position(|h| h == "Boundary Name") {
            Some(i) => i,
            None => return,
        };
        let boundary_type_idx = match headers.iter().position(|h| h == "Boundary Type") {
            Some(i) => i,
            None => return,
        };
        let material_name_idx = match headers.iter().position(|h| h == "Material Name") {
            Some(i) => i,
            None => return,
        };
        let resistance_idx = match headers.iter().position(|h| h == "Resistance (m^2-K/W)") {
            Some(i) => i,
            None => return,
        };
        let conductivity_idx = match headers.iter().position(|h| h == "Conductivity (W/m-K)") {
            Some(i) => i,
            None => return,
        };

        // Group rows by (boundary_name, variant_prefix) where variant_prefix is
        // the portion of the Boundary Type that excludes the R-value label.
        // This ensures we only compare variants that differ in insulation level,
        // not in construction type or siding material.
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<(String, String), BTreeMap<u32, (f64, f64)>> = BTreeMap::new();
        //     (boundary_name, prefix) → insulation_rank → (resistance, conductivity)

        for result in rdr.records() {
            let record = match result {
                Ok(r) => r,
                Err(_) => continue,
            };
            let material = record.get(material_name_idx).unwrap_or("");
            if !material.contains("STUD AND CAVITY") {
                continue;
            }
            let boundary_name = record.get(boundary_name_idx).unwrap_or("");
            let boundary_type = record.get(boundary_type_idx).unwrap_or("");
            let r_str = record.get(resistance_idx).unwrap_or("");
            let k_str = record.get(conductivity_idx).unwrap_or("");

            if boundary_name.is_empty()
                || boundary_type.is_empty()
                || r_str.is_empty()
                || k_str.is_empty()
            {
                continue;
            }
            let r: f64 = match r_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let k: f64 = match k_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Split variant into (prefix, insulation_rank).
            // "Ceiling R-13" → ("Ceiling ", 13)
            // "WoodStud, aluminum siding, R-11" → ("WoodStud, aluminum siding, ", 11)
            // "Uninsulated" → ("", 0)
            // "Minimal" or unrecognised → skip
            let (prefix, rank) = match split_r_variant(boundary_type) {
                Some(v) => v,
                None => continue,
            };

            let key = (boundary_name.to_string(), prefix.to_string());
            groups.entry(key).or_default().insert(rank, (r, k));
        }

        // Check monotonicity within each (boundary_name, prefix) group.
        for ((boundary_name, prefix), variants) in &groups {
            if variants.len() < 2 {
                continue;
            }
            let ordered: Vec<(u32, f64, f64)> = variants
                .iter()
                .map(|(rank, &(r, k))| (*rank, r, k))
                .collect();
            // Already sorted by BTreeMap key.

            for window in ordered.windows(2) {
                let (prev_rank, prev_r, _prev_k) = window[0];
                let (next_rank, next_r, _next_k) = window[1];
                if next_r < prev_r {
                    tracing::warn!(
                        boundary = %boundary_name,
                        prefix = %prefix,
                        from_rank = prev_rank,
                        to_rank = next_rank,
                        from_r = prev_r,
                        to_r = next_r,
                        "Non-monotonic stud cavity R-value within variant group: rank {} (R={:.4}) → rank {} (R={:.4})",
                        prev_rank,
                        prev_r,
                        next_rank,
                        next_r,
                    );
                }
            }

            // Check conductivities: flag insulated stud-cavity layers with
            // implausibly high k (> 0.5 W/m·K). Uninsulated cavities (rank 0)
            // naturally have higher k due to air convection and are excluded.
            // ASHRAE HoF 2021 Ch.26: still air k ≈ 0.026 W/m·K. An insulated
            // stud cavity typically has k_eff ≈ 0.04–0.10 W/m·K. Values above
            // 0.5 W/m·K for an insulated cavity indicate a back-calculation
            // forced the layer to act as a near-zero-resistance thermal short.
            const MAX_PLAUSIBLE_CAVITY_K: f64 = 0.5;
            for (&rank, &(_r, k)) in variants.iter() {
                if rank > 0 && k > MAX_PLAUSIBLE_CAVITY_K {
                    tracing::warn!(
                        boundary = %boundary_name,
                        prefix = %prefix,
                        rank = rank,
                        conductivity = k,
                        threshold = MAX_PLAUSIBLE_CAVITY_K,
                        "Insulated stud cavity conductivity {:.4} W/m·K exceeds plausible maximum {:.4} W/m·K; back-calculation may have produced physically wrong per-layer properties",
                        k,
                        MAX_PLAUSIBLE_CAVITY_K,
                    );
                }
            }
        }
    }

    /// Implements OCHRE's `get_boundary_rc_values` matching algorithm.
    pub fn lookup(
        &self,
        boundary_name: &str,
        construction_type: Option<&str>,
        finish_type: Option<&str>,
        insulation_details: Option<&str>,
        assembly_r_value: Option<f64>,
    ) -> Option<EnvelopeLookupResult> {
        let candidates = self.boundary_types.get(boundary_name)?;
        if candidates.is_empty() {
            return None;
        }

        // Progressive filtering
        let mut filtered: Vec<&BoundaryTypeRow> = candidates.iter().collect();

        if let Some(ct) = construction_type {
            if !ct.is_empty() {
                let narrowed: Vec<_> = filtered
                    .iter()
                    .filter(|r| r.construction_type == ct)
                    .copied()
                    .collect();
                if !narrowed.is_empty() {
                    filtered = narrowed;
                }
            }
        }

        if let Some(ft) = finish_type {
            if !ft.is_empty() {
                let narrowed: Vec<_> = filtered
                    .iter()
                    .filter(|r| r.finish_type == ft)
                    .copied()
                    .collect();
                if !narrowed.is_empty() {
                    filtered = narrowed;
                }
            }
        }

        if let Some(ins) = insulation_details {
            if !ins.is_empty() {
                let narrowed: Vec<_> = filtered
                    .iter()
                    .filter(|r| r.insulation_details == ins)
                    .copied()
                    .collect();
                if !narrowed.is_empty() {
                    filtered = narrowed;
                }
            }
        }

        if filtered.is_empty() {
            return None;
        }

        let matched = if filtered.len() == 1 {
            filtered[0]
        } else if let Some(r_val) = assembly_r_value {
            // OCHRE clamps R >= 17.6 m²·K/W (100 IP) to ~88 m²·K/W (500 IP) for "Minimal" boundaries.
            // When clamping, clear all type filters so the Minimal row is reachable.
            let mut r_val = r_val;
            if r_val >= 17.6 {
                filtered = candidates.iter().collect();
                r_val = 88.0;
            }

            let film_r = if FLOOR_CEILING_BOUNDARIES.contains(&boundary_name) {
                FILM_R_FLOOR_CEILING
            } else {
                FILM_R_WALL_ROOF
            };

            // Find closest adjusted R-value
            filtered
                .iter()
                .min_by(|a, b| {
                    let da = ((a.assembly_r_value + film_r) - r_val).abs();
                    let db = ((b.assembly_r_value + film_r) - r_val).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()?
        } else {
            filtered[0]
        };

        // Extract material layers
        let key = (boundary_name.to_string(), matched.boundary_type.clone());
        let mat_rows = self.materials.get(&key)?;
        if mat_rows.is_empty() {
            return None;
        }

        let layers: Vec<PrecomputedLayer> = mat_rows
            .iter()
            .map(|m| PrecomputedLayer {
                resistance_m2_k_w: m.resistance,
                capacitance_kj_m2_k: m.capacitance,
            })
            .collect();

        Some(EnvelopeLookupResult {
            layers,
            matched_boundary_type: matched.boundary_type.clone(),
            matched_r_value: matched.assembly_r_value,
        })
    }

    fn load_boundary_types(
        path: &Path,
    ) -> Result<HashMap<String, Vec<BoundaryTypeRow>>, EnvelopeLutError> {
        let mut rdr = csv::Reader::from_path(path).map_err(|e| EnvelopeLutError::CsvParse {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let mut map: HashMap<String, Vec<BoundaryTypeRow>> = HashMap::new();
        for result in rdr.deserialize() {
            let row: BoundaryTypeRow = result.map_err(|e| EnvelopeLutError::CsvParse {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            map.entry(row.boundary_name.clone()).or_default().push(row);
        }
        Ok(map)
    }

    fn load_materials(
        path: &Path,
    ) -> Result<HashMap<(String, String), Vec<MaterialRow>>, EnvelopeLutError> {
        let mut rdr = csv::Reader::from_path(path).map_err(|e| EnvelopeLutError::CsvParse {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let mut map: HashMap<(String, String), Vec<MaterialRow>> = HashMap::new();
        for result in rdr.deserialize() {
            let row: MaterialRow = result.map_err(|e| EnvelopeLutError::CsvParse {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
            let key = (row.boundary_name.clone(), row.boundary_type.clone());
            map.entry(key).or_default().push(row);
        }
        Ok(map)
    }

    /// Parse `Envelope Boundaries.csv` into a `(BoundaryType, ZoneType, ZoneType) → boundary_name` map.
    fn load_boundaries(
        path: &Path,
    ) -> Result<HashMap<(BoundaryType, ZoneType, ZoneType), String>, EnvelopeLutError> {
        let mut rdr = csv::Reader::from_path(path).map_err(|e| EnvelopeLutError::CsvParse {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let mut map: HashMap<(BoundaryType, ZoneType, ZoneType), String> = HashMap::new();
        for result in rdr.deserialize() {
            let row: BoundaryRow = result.map_err(|e| EnvelopeLutError::CsvParse {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;

            let ext = match zone_label_to_type(&row.exterior_zone_label) {
                Some(z) => z,
                None => {
                    tracing::warn!(
                        label = %row.exterior_zone_label,
                        boundary = %row.boundary_name,
                        "unrecognized exterior zone label in Envelope Boundaries.csv; skipping row"
                    );
                    continue;
                }
            };
            let int = match zone_label_to_type(&row.interior_zone_label) {
                Some(z) => z,
                None => {
                    tracing::warn!(
                        label = %row.interior_zone_label,
                        boundary = %row.boundary_name,
                        "unrecognized interior zone label in Envelope Boundaries.csv; skipping row"
                    );
                    continue;
                }
            };
            let bt = match boundary_type_from_boundary_name(&row.boundary_name) {
                Some(b) => b,
                None => {
                    // Furniture and other non-boundary entries are expected
                    // and deliberately excluded from the lookup table.
                    continue;
                }
            };

            // Adjacent zone resolution is handled by the hardcoded path
            // (resolve_name / resolve_boundary_name), not the CSV-derived map.
            // The Adjacent rows in the CSV use same-zone pairs (LIV/LIV, FND/FND etc.)
            // because OCHRE's CSV schema has no Adjacent zone label. Inserting them
            // as (BoundaryType, ZoneType, ZoneType) entries would silently overwrite
            // interior/conditioned-space entries with no observable effect (the
            // early-return guards in resolve_name intercept every Adjacent key before
            // the CSV map is consulted). Skip them explicitly so the map accurately
            // reflects the architecture: the CSV handles non-adjacent boundaries;
            // the hardcoded path handles adjacent ones.
            if row.boundary_name.starts_with("Adjacent") {
                tracing::debug!(
                    boundary = %row.boundary_name,
                    "skipping Adjacent boundary row; adjacent zone resolution is handled by the hardcoded path, not the CSV"
                );
                continue;
            }

            // Key ordering matches resolve_boundary_name: (boundary_type, interior, exterior).
            let key = (bt.clone(), int.clone(), ext.clone());
            if let Some(previous) = map.insert(key, row.boundary_name.clone()) {
                tracing::warn!(
                    key = ?(bt, int, ext),
                    previous = %previous,
                    new = %row.boundary_name,
                    "boundary name collision in Envelope Boundaries.csv; overwriting entry"
                );
            }
        }
        Ok(map)
    }
}

// ── Boundary name resolution ────────────────────────────────────────────────

/// Map HPXML boundary type + zone info to OCHRE boundary name.
///
/// Returns `None` for unmapped combinations (e.g. Window, which has its own
/// U-factor path).
pub fn resolve_boundary_name(
    boundary_type: &BoundaryType,
    interior_zone: Option<&ZoneType>,
    exterior_zone: Option<&ZoneType>,
) -> Option<&'static str> {
    // Use sensible defaults when zones are missing -- OCHRE infers these
    // from boundary type conventions.
    let default_int = ZoneType::Conditioned;
    let default_ext = match boundary_type {
        BoundaryType::Roof | BoundaryType::Wall | BoundaryType::Door | BoundaryType::RimJoist => {
            ZoneType::Outdoor
        }
        BoundaryType::Slab | BoundaryType::FoundationWall => ZoneType::Ground,
        BoundaryType::Floor => ZoneType::Attic,
        _ => ZoneType::Outdoor,
    };
    let int = interior_zone.unwrap_or(&default_int);
    let ext = exterior_zone.unwrap_or(&default_ext);

    // Same-zone boundaries: furniture (thermal mass) or adjacent (adiabatic).
    // OCHRE hpxml.py:763-777 (furniture), 374-380 (adjacent).
    if int == ext {
        return match int {
            ZoneType::Conditioned => Some("Indoor Furniture"),
            ZoneType::Foundation => Some("Foundation Furniture"),
            ZoneType::Garage => Some("Garage Furniture"),
            _ => None,
        };
    }

    // Adjacent (adiabatic) boundaries -- multifamily party walls/floors.
    if *int == ZoneType::Adjacent || *ext == ZoneType::Adjacent {
        let other = if *int == ZoneType::Adjacent { ext } else { int };
        return match (boundary_type, other) {
            (BoundaryType::Wall, ZoneType::Attic) => Some("Adjacent Attic Wall"),
            (BoundaryType::Wall, ZoneType::Garage) => Some("Adjacent Wall"),
            (BoundaryType::Wall, _) => Some("Adjacent Wall"),
            (BoundaryType::Floor, _) => Some("Adjacent Floor"),
            (BoundaryType::Roof, _) => Some("Adjacent Ceiling"),
            (BoundaryType::FoundationWall, _) => Some("Adjacent Foundation Wall"),
            (BoundaryType::RimJoist, _) => Some("Adjacent Rim Joist"),
            _ => None,
        };
    }

    match boundary_type {
        BoundaryType::Wall => match (int, ext) {
            (ZoneType::Conditioned, ZoneType::Outdoor) => Some("Exterior Wall"),
            (ZoneType::Conditioned, ZoneType::Conditioned) => Some("Interior Wall"),
            (ZoneType::Attic, ZoneType::Outdoor) => Some("Attic Wall"),
            (ZoneType::Garage, ZoneType::Outdoor) => Some("Garage Wall"),
            (ZoneType::Garage, ZoneType::Conditioned)
            | (ZoneType::Conditioned, ZoneType::Garage) => Some("Garage Attached Wall"),
            (ZoneType::Foundation, ZoneType::Outdoor) => Some("Exterior Wall"),
            _ => Some("Exterior Wall"),
        },
        BoundaryType::Roof => match (int, ext) {
            (ZoneType::Attic, ZoneType::Outdoor) => Some("Attic Roof"),
            (ZoneType::Garage, ZoneType::Outdoor) => Some("Garage Roof"),
            (ZoneType::Conditioned, ZoneType::Outdoor) => Some("Roof"),
            _ => Some("Attic Roof"),
        },
        BoundaryType::Floor => match (int, ext) {
            (ZoneType::Conditioned, ZoneType::Attic) => Some("Attic Floor"),
            (ZoneType::Conditioned, ZoneType::Foundation) => Some("Foundation Ceiling"),
            (ZoneType::Conditioned, ZoneType::Garage) => Some("Garage Interior Ceiling"),
            (ZoneType::Garage, ZoneType::Attic) => Some("Garage Ceiling"),
            (ZoneType::Conditioned, ZoneType::Outdoor) => Some("Raised Floor"),
            _ => Some("Attic Floor"),
        },
        BoundaryType::Slab => match (int, ext) {
            (ZoneType::Foundation, _) => Some("Foundation Floor"),
            (ZoneType::Conditioned, _) => Some("Floor"),
            (ZoneType::Garage, _) => Some("Garage Floor"),
            _ => Some("Foundation Floor"),
        },
        BoundaryType::FoundationWall => Some("Foundation Wall"),
        BoundaryType::RimJoist => Some("Rim Joist"),
        BoundaryType::Door => match (int, ext) {
            (ZoneType::Garage, ZoneType::Outdoor) => Some("Garage Door"),
            _ => Some("Door"),
        },
        BoundaryType::Window | BoundaryType::Other(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hpxml::{BoundaryType, ZoneType};

    #[test]
    fn raised_floor_boundary_name() {
        assert_eq!(
            resolve_boundary_name(
                &BoundaryType::Floor,
                Some(&ZoneType::Conditioned),
                Some(&ZoneType::Outdoor),
            ),
            Some("Raised Floor")
        );
    }

    #[test]
    fn attic_floor_boundary_name() {
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
    fn foundation_ceiling_boundary_name() {
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
    fn slab_on_grade_boundary_name() {
        assert_eq!(
            resolve_boundary_name(
                &BoundaryType::Slab,
                Some(&ZoneType::Conditioned),
                Some(&ZoneType::Ground),
            ),
            Some("Floor")
        );
    }

    #[test]
    fn foundation_wall_boundary_name() {
        assert_eq!(
            resolve_boundary_name(
                &BoundaryType::FoundationWall,
                Some(&ZoneType::Foundation),
                Some(&ZoneType::Ground),
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
}
