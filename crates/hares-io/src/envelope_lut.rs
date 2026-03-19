//! Envelope LUT parser for loading OCHRE's pre-computed RC values from CSV files.
//!
//! Parses three CSV files from the `defaults/envelope/` directory:
//! - `Envelope Boundaries.csv` — zone label mappings per boundary name
//! - `Envelope Boundary Types.csv` — construction variants with assembly R-values
//! - `Envelope Materials.csv` — per-layer resistance and capacitance values

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

/// Loaded and indexed envelope lookup tables.
#[derive(Debug, Clone)]
pub struct EnvelopeLookup {
    /// Boundary type rows grouped by boundary name.
    boundary_types: HashMap<String, Vec<BoundaryTypeRow>>,
    /// Material rows grouped by (boundary_name, boundary_type).
    materials: HashMap<(String, String), Vec<MaterialRow>>,
}

impl EnvelopeLookup {
    /// Parse the three envelope CSV files from `dir`.
    pub fn load(dir: &Path) -> Result<Self, EnvelopeLutError> {
        let bt_path = dir.join("Envelope Boundary Types.csv");
        let mat_path = dir.join("Envelope Materials.csv");

        if !bt_path.exists() {
            return Err(EnvelopeLutError::MissingFile(bt_path));
        }
        if !mat_path.exists() {
            return Err(EnvelopeLutError::MissingFile(mat_path));
        }

        let boundary_types = Self::load_boundary_types(&bt_path)?;
        let materials = Self::load_materials(&mat_path)?;

        Ok(Self {
            boundary_types,
            materials,
        })
    }

    /// Look up pre-computed RC layers for a boundary.
    ///
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
            let r_val = if r_val >= 17.6 { 88.0 } else { r_val };

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
    // Use sensible defaults when zones are missing — OCHRE infers these
    // from boundary type conventions.
    let default_int = ZoneType::Conditioned;
    let default_ext = match boundary_type {
        BoundaryType::Roof | BoundaryType::Wall | BoundaryType::Door | BoundaryType::RimJoist => {
            ZoneType::Outdoor
        }
        BoundaryType::Slab | BoundaryType::FoundationWall => ZoneType::Outdoor,
        BoundaryType::Floor => ZoneType::Attic,
        _ => ZoneType::Outdoor,
    };
    let int = interior_zone.unwrap_or(&default_int);
    let ext = exterior_zone.unwrap_or(&default_ext);

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
        BoundaryType::Floor => {
            // HPXML <Floor> elements represent horizontal separations.
            // The exterior zone determines which OCHRE boundary this maps to.
            match (int, ext) {
                (ZoneType::Conditioned, ZoneType::Attic) => Some("Attic Floor"),
                (ZoneType::Conditioned, ZoneType::Foundation) => Some("Foundation Ceiling"),
                (ZoneType::Conditioned, ZoneType::Garage) => Some("Garage Interior Ceiling"),
                (ZoneType::Garage, ZoneType::Attic) => Some("Garage Ceiling"),
                _ => Some("Attic Floor"),
            }
        }
        BoundaryType::Slab => match (int, ext) {
            (ZoneType::Foundation, _) => Some("Foundation Floor"),
            (ZoneType::Conditioned, _) => Some("Floor"),
            (ZoneType::Garage, _) => Some("Garage Floor"),
            _ => Some("Foundation Floor"),
        },
        BoundaryType::FoundationWall => Some("Foundation Wall"),
        BoundaryType::RimJoist => Some("Rim Joist"),
        BoundaryType::Door => Some("Door"),
        BoundaryType::Window | BoundaryType::Other(_) => None,
    }
}
