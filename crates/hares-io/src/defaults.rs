//! Default parameter loading from the `defaults/` directory tree.
//!
//! The [`DefaultsStore`] loads equipment ZIP parameters and HVAC biquadratic
//! coefficient sets at startup so that equipment specs can be fully resolved
//! before simulation begins.
//!
//! OCHRE defaults mapping (source -> HARES entry):
//! - `ochre/defaults/ZIP Parameters.csv` -> `defaults/zip_parameters.toml` -> [`ZipParameters`]
//! - `ochre/defaults/HVAC Cooling/Biquadratic *.csv` -> `defaults/hvac_cooling/*.toml` -> [`HvacCurveSet`]
//! - `ochre/defaults/HVAC Heating/Biquadratic *.csv` -> `defaults/hvac_heating/*.toml` -> [`HvacCurveSet`]
//! - `ochre/defaults/Battery/*` -> `defaults/battery/*.toml`
//! - `ochre/defaults/Envelope/*` -> `defaults/envelope/*.toml`
//! - `ochre/defaults/EV/*` -> `defaults/ev/*.toml`
//! - `ochre/defaults/Gas Generator/*` -> `defaults/generator/*.toml`
//! - `ochre/defaults/PV/*` -> `defaults/pv/*.toml`
//! - `ochre/defaults/Water Heating/*` -> `defaults/water_heating/*.toml`
//! - appliance and event schedule defaults -> `defaults/loads/*.toml`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hares_physics::biquadratic::BiquadraticCurve;
use serde::Deserialize;
use thiserror::Error;

/// ZIP load model parameters for voltage-dependent power modelling.
///
/// Real power: `P(V) = P0 * [zp*(V/V0)^2 + ip*(V/V0) + pp]`
/// Reactive power: `Q(V) = Q0 * [zq*(V/V0)^2 + iq*(V/V0) + pq]`
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ZipParameters {
    /// Impedance fraction (real power, voltage-squared term).
    pub zp: f64,
    /// Current fraction (real power, voltage-linear term).
    pub ip: f64,
    /// Power fraction (real power, constant term).
    pub pp: f64,
    /// Impedance fraction (reactive power, voltage-squared term).
    pub zq: f64,
    /// Current fraction (reactive power, voltage-linear term).
    pub iq: f64,
    /// Power fraction (reactive power, constant term).
    pub pq: f64,
    /// Power factor.
    pub pf: f64,
}

/// A named set of biquadratic curves for one HVAC speed variant.
///
/// Each variant (e.g. "Single_1", "Variable_2") contains curves for
/// capacity and EIR as functions of temperature, flow fraction, and
/// part-load ratio, plus operating bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct HvacCurveVariant {
    pub name: String,
    pub cap_t: BiquadraticCurve,
    pub cap_ff: [f64; 3],
    pub eir_t: BiquadraticCurve,
    pub eir_ff: [f64; 3],
    pub eir_plr: [f64; 3],
}

/// Collection of HVAC biquadratic curve variants for one equipment type.
#[derive(Debug, Clone, PartialEq)]
pub struct HvacCurveSet {
    pub variants: Vec<HvacCurveVariant>,
}

/// One row from `defaults/HVAC Multispeed Parameters.csv`.
#[derive(Debug, Clone, PartialEq)]
pub struct HvacMultispeedParameters {
    pub hvac_name: String,
    pub efficiency_kind: String,
    pub efficiency_value: f64,
    pub number_of_speeds: usize,
    pub capacity_ratios: Vec<f64>,
    pub airflow_ratios: Vec<f64>,
    pub cops: Vec<f64>,
    pub shrs: Vec<f64>,
}

/// API alias for HVAC coefficient lookups.
pub type BiquadraticCoefficients = HvacCurveSet;

/// Generic default parameters loaded from a TOML file in any equipment
/// subdirectory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct EquipmentDefaults {
    pub params: HashMap<String, toml::Value>,
}

/// Central store for all default parameters loaded from the `defaults/` tree.
#[derive(Debug, Clone, Default)]
pub struct DefaultsStore {
    zip_by_equipment: HashMap<String, ZipParameters>,
    hvac_cooling: HashMap<String, HvacCurveSet>,
    hvac_heating: HashMap<String, HvacCurveSet>,
    hvac_multispeed: Vec<HvacMultispeedParameters>,
    battery: HashMap<String, EquipmentDefaults>,
    envelope: HashMap<String, EquipmentDefaults>,
    ev: HashMap<String, EquipmentDefaults>,
    generator: HashMap<String, EquipmentDefaults>,
    loads: HashMap<String, EquipmentDefaults>,
    pv: HashMap<String, EquipmentDefaults>,
    water_heating: HashMap<String, EquipmentDefaults>,
    envelope_lut: Option<crate::envelope_lut::EnvelopeLookup>,
}

#[derive(Debug, Error)]
pub enum DefaultsError {
    #[error("missing defaults file: {0}")]
    MissingFile(PathBuf),
    #[error("malformed TOML in {path}: {reason}")]
    MalformedToml { path: PathBuf, reason: String },
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl DefaultsStore {
    /// Create an empty store (useful for tests without filesystem fixtures).
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Load all default parameters from the given directory tree.
    ///
    /// Expected layout:
    /// ```text
    /// defaults/
    ///   zip_parameters.toml
    ///   hvac_cooling/
    ///   hvac_heating/
    ///   battery/
    ///   envelope/
    ///   ev/
    ///   generator/
    ///   loads/
    ///   pv/
    ///   water_heating/
    /// ```
    pub fn load(defaults_dir: &Path) -> Result<Self, DefaultsError> {
        let mut store = Self::default();

        let zip_path = defaults_dir.join("zip_parameters.toml");
        if !zip_path.exists() {
            return Err(DefaultsError::MissingFile(zip_path));
        }
        store.zip_by_equipment = load_zip_parameters(&zip_path)?;

        store.hvac_cooling = load_hvac_curves_dir(&defaults_dir.join("hvac_cooling"))?;
        store.hvac_heating = load_hvac_curves_dir(&defaults_dir.join("hvac_heating"))?;
        store.hvac_multispeed =
            load_hvac_multispeed_csv(&defaults_dir.join("HVAC Multispeed Parameters.csv"))?;

        // Load envelope LUT from CSV files (non-fatal if missing).
        let envelope_dir = defaults_dir.join("envelope");
        match crate::envelope_lut::EnvelopeLookup::load(&envelope_dir) {
            Ok(lut) => store.envelope_lut = Some(lut),
            Err(err) => {
                tracing::warn!(%err, "envelope LUT load failed, falling back to material layers");
            }
        }

        store.battery = load_toml_dir(&defaults_dir.join("battery"))?;
        store.envelope = load_toml_dir(&defaults_dir.join("envelope"))?;
        store.ev = load_toml_dir(&defaults_dir.join("ev"))?;
        store.generator = load_toml_dir(&defaults_dir.join("generator"))?;
        store.loads = load_toml_dir(&defaults_dir.join("loads"))?;
        store.pv = load_toml_dir(&defaults_dir.join("pv"))?;
        store.water_heating = load_toml_dir(&defaults_dir.join("water_heating"))?;

        Ok(store)
    }

    /// Look up ZIP parameters by equipment type name.
    ///
    /// Name matching is canonicalized to lowercase snake case.
    #[must_use]
    pub fn zip_params(&self, equipment_type: &str) -> Option<&ZipParameters> {
        self.zip_by_equipment
            .get(&normalize_equipment_key(equipment_type))
    }

    /// Look up HVAC cooling biquadratic coefficient set by equipment type.
    #[must_use]
    pub fn hvac_cooling_coefficients(
        &self,
        equipment_type: &str,
    ) -> Option<&BiquadraticCoefficients> {
        self.hvac_cooling
            .get(&normalize_equipment_key(equipment_type))
    }

    /// Look up HVAC heating biquadratic coefficient set by equipment type.
    #[must_use]
    pub fn hvac_heating_coefficients(
        &self,
        equipment_type: &str,
    ) -> Option<&BiquadraticCoefficients> {
        self.hvac_heating
            .get(&normalize_equipment_key(equipment_type))
    }

    /// Look up HVAC cooling biquadratic curve set by equipment type.
    #[must_use]
    pub fn hvac_cooling_curves(&self, equipment_type: &str) -> Option<&HvacCurveSet> {
        self.hvac_cooling
            .get(&normalize_equipment_key(equipment_type))
    }

    /// Look up HVAC heating biquadratic curve set by equipment type.
    #[must_use]
    pub fn hvac_heating_curves(&self, equipment_type: &str) -> Option<&HvacCurveSet> {
        self.hvac_heating
            .get(&normalize_equipment_key(equipment_type))
    }

    /// Find the closest multispeed parameter row by efficiency for one HVAC type.
    #[must_use]
    pub fn hvac_multispeed_parameters(
        &self,
        equipment_type: &str,
        efficiency_kind: &str,
        number_of_speeds: usize,
        efficiency_value: f64,
    ) -> Option<&HvacMultispeedParameters> {
        let eq_key = normalize_equipment_key(equipment_type);
        let eff_key = efficiency_kind.trim().to_ascii_uppercase();

        self.hvac_multispeed
            .iter()
            .filter(|row| {
                normalize_equipment_key(&row.hvac_name) == eq_key
                    && row.number_of_speeds == number_of_speeds
                    && row.efficiency_kind.eq_ignore_ascii_case(&eff_key)
            })
            .min_by(|a, b| {
                let da = (a.efficiency_value - efficiency_value).abs();
                let db = (b.efficiency_value - efficiency_value).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Look up generic defaults for a category and equipment type.
    #[must_use]
    pub fn equipment_defaults(
        &self,
        category: DefaultsCategory,
        equipment_type: &str,
    ) -> Option<&EquipmentDefaults> {
        let map = match category {
            DefaultsCategory::Battery => &self.battery,
            DefaultsCategory::Envelope => &self.envelope,
            DefaultsCategory::Ev => &self.ev,
            DefaultsCategory::Generator => &self.generator,
            DefaultsCategory::Loads => &self.loads,
            DefaultsCategory::Pv => &self.pv,
            DefaultsCategory::WaterHeating => &self.water_heating,
        };
        map.get(&normalize_equipment_key(equipment_type))
    }

    /// Access the envelope LUT (if loaded successfully).
    #[must_use]
    pub fn envelope_lut(&self) -> Option<&crate::envelope_lut::EnvelopeLookup> {
        self.envelope_lut.as_ref()
    }

    /// Number of ZIP parameter entries loaded.
    #[must_use]
    pub fn zip_count(&self) -> usize {
        self.zip_by_equipment.len()
    }

    /// Number of entries loaded for one generic defaults category.
    #[must_use]
    pub fn category_count(&self, category: DefaultsCategory) -> usize {
        match category {
            DefaultsCategory::Battery => self.battery.len(),
            DefaultsCategory::Envelope => self.envelope.len(),
            DefaultsCategory::Ev => self.ev.len(),
            DefaultsCategory::Generator => self.generator.len(),
            DefaultsCategory::Loads => self.loads.len(),
            DefaultsCategory::Pv => self.pv.len(),
            DefaultsCategory::WaterHeating => self.water_heating.len(),
        }
    }
}

/// Equipment default subdirectory categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultsCategory {
    Battery,
    Envelope,
    Ev,
    Generator,
    Loads,
    Pv,
    WaterHeating,
}

// ---------------------------------------------------------------------------
// Internal loaders
// ---------------------------------------------------------------------------

fn load_zip_parameters(path: &Path) -> Result<HashMap<String, ZipParameters>, DefaultsError> {
    let content = std::fs::read_to_string(path).map_err(|e| DefaultsError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let table: HashMap<String, ZipParameters> =
        toml::from_str(&content).map_err(|e| DefaultsError::MalformedToml {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

    Ok(table
        .into_iter()
        .map(|(name, params)| (normalize_equipment_key(&name), params))
        .collect())
}

fn load_toml_dir(dir: &Path) -> Result<HashMap<String, EquipmentDefaults>, DefaultsError> {
    let mut map = HashMap::new();
    if !dir.exists() {
        return Ok(map);
    }
    let entries = std::fs::read_dir(dir).map_err(|e| DefaultsError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| DefaultsError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            let stem =
                normalize_equipment_key(&path.file_stem().unwrap_or_default().to_string_lossy());
            let content = std::fs::read_to_string(&path).map_err(|e| DefaultsError::Io {
                path: path.clone(),
                source: e,
            })?;
            let defaults: EquipmentDefaults =
                toml::from_str(&content).map_err(|e| DefaultsError::MalformedToml {
                    path: path.clone(),
                    reason: e.to_string(),
                })?;
            map.insert(stem, defaults);
        }
    }
    Ok(map)
}

fn load_hvac_curves_dir(dir: &Path) -> Result<HashMap<String, HvacCurveSet>, DefaultsError> {
    let mut map = HashMap::new();
    if !dir.exists() {
        return Ok(map);
    }
    let entries = std::fs::read_dir(dir).map_err(|e| DefaultsError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| DefaultsError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        let ext = path.extension().map(|e| e.to_string_lossy().to_string());
        let stem = normalize_equipment_key(&path.file_stem().unwrap_or_default().to_string_lossy());

        match ext.as_deref() {
            Some("toml") => {
                let curve_set = load_hvac_curve_file(&path)?;
                map.insert(stem, curve_set);
            }
            Some("csv") => match load_hvac_csv_file(&path) {
                Ok(curve_set) => {
                    map.insert(stem, curve_set);
                }
                Err(err) => {
                    tracing::warn!(path = %path.display(), %err, "failed to parse HVAC CSV");
                }
            },
            _ => {}
        }
    }
    Ok(map)
}

/// TOML structure for a single HVAC curve variant.
#[derive(Debug, Deserialize)]
struct RawHvacVariant {
    name: String,
    /// Biquadratic capacity-temperature coefficients [a, b, c, d, e, f].
    cap_t: [f64; 6],
    /// Quadratic capacity-flow-fraction coefficients [a, b, c].
    cap_ff: [f64; 3],
    /// Biquadratic EIR-temperature coefficients [a, b, c, d, e, f].
    eir_t: [f64; 6],
    /// Quadratic EIR-flow-fraction coefficients [a, b, c].
    eir_ff: [f64; 3],
    /// Quadratic EIR-part-load-ratio coefficients [a, b, c].
    eir_plr: [f64; 3],
    /// [min, max] wet-bulb temperature bounds (deg C).
    twb_bounds: [f64; 2],
    /// [min, max] dry-bulb temperature bounds (deg C).
    tdb_bounds: [f64; 2],
}

#[derive(Debug, Deserialize)]
struct RawHvacFile {
    variant: Vec<RawHvacVariant>,
}

fn load_hvac_curve_file(path: &Path) -> Result<HvacCurveSet, DefaultsError> {
    let content = std::fs::read_to_string(path).map_err(|e| DefaultsError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let raw: RawHvacFile = toml::from_str(&content).map_err(|e| DefaultsError::MalformedToml {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    let variants = raw
        .variant
        .into_iter()
        .map(|v| HvacCurveVariant {
            name: v.name,
            cap_t: BiquadraticCurve {
                coeffs: v.cap_t,
                x1_bounds: (v.twb_bounds[0], v.twb_bounds[1]),
                x2_bounds: (v.tdb_bounds[0], v.tdb_bounds[1]),
            },
            cap_ff: v.cap_ff,
            eir_t: BiquadraticCurve {
                coeffs: v.eir_t,
                x1_bounds: (v.twb_bounds[0], v.twb_bounds[1]),
                x2_bounds: (v.tdb_bounds[0], v.tdb_bounds[1]),
            },
            eir_ff: v.eir_ff,
            eir_plr: v.eir_plr,
        })
        .collect();
    Ok(HvacCurveSet { variants })
}

/// Parse OCHRE-format CSV biquadratic curves.
///
/// OCHRE CSV format: rows are coefficient names, columns are speed variants (transposed).
/// Row names: a_eir_t..f_eir_t, a_eir_ff..c_eir_ff, a_eir_plr..c_eir_plr,
///            a_cap_t..f_cap_t, a_cap_ff..c_cap_ff, min_Twb, max_Twb, min_Tdb, max_Tdb
fn load_hvac_csv_file(path: &Path) -> Result<HvacCurveSet, DefaultsError> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| DefaultsError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;

    let headers = rdr
        .headers()
        .map_err(|e| DefaultsError::MalformedToml {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?
        .clone();

    // First column is "Name", remaining are variant names.
    let variant_names: Vec<String> = headers.iter().skip(1).map(|s| s.to_string()).collect();
    let n_variants = variant_names.len();

    // Read all rows into a map: row_name → Vec<f64> (one per variant).
    let mut data: HashMap<String, Vec<f64>> = HashMap::new();
    for result in rdr.records() {
        let record = result.map_err(|e| DefaultsError::MalformedToml {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        let row_name = record.get(0).unwrap_or("").to_string();
        let values: Vec<f64> = (1..=n_variants)
            .map(|i| record.get(i).unwrap_or("0").parse::<f64>().unwrap_or(0.0))
            .collect();
        data.insert(row_name, values);
    }

    let get_row = |name: &str| -> Vec<f64> {
        data.get(name)
            .cloned()
            .unwrap_or_else(|| vec![0.0; n_variants])
    };

    let variants: Vec<HvacCurveVariant> = (0..n_variants)
        .map(|i| {
            let eir_t = [
                get_row("a_eir_t")[i],
                get_row("b_eir_t")[i],
                get_row("c_eir_t")[i],
                get_row("d_eir_t")[i],
                get_row("e_eir_t")[i],
                get_row("f_eir_t")[i],
            ];
            let cap_t = [
                get_row("a_cap_t")[i],
                get_row("b_cap_t")[i],
                get_row("c_cap_t")[i],
                get_row("d_cap_t")[i],
                get_row("e_cap_t")[i],
                get_row("f_cap_t")[i],
            ];
            let twb_min = get_row("min_Twb")[i];
            let twb_max = get_row("max_Twb")[i];
            let tdb_min = get_row("min_Tdb")[i];
            let tdb_max = get_row("max_Tdb")[i];

            HvacCurveVariant {
                name: variant_names[i].clone(),
                cap_t: BiquadraticCurve {
                    coeffs: cap_t,
                    x1_bounds: (twb_min, twb_max),
                    x2_bounds: (tdb_min, tdb_max),
                },
                cap_ff: [
                    get_row("a_cap_ff")[i],
                    get_row("b_cap_ff")[i],
                    get_row("c_cap_ff")[i],
                ],
                eir_t: BiquadraticCurve {
                    coeffs: eir_t,
                    x1_bounds: (twb_min, twb_max),
                    x2_bounds: (tdb_min, tdb_max),
                },
                eir_ff: [
                    get_row("a_eir_ff")[i],
                    get_row("b_eir_ff")[i],
                    get_row("c_eir_ff")[i],
                ],
                eir_plr: [
                    get_row("a_eir_plr")[i],
                    get_row("b_eir_plr")[i],
                    get_row("c_eir_plr")[i],
                ],
            }
        })
        .collect();

    Ok(HvacCurveSet { variants })
}

fn load_hvac_multispeed_csv(path: &Path) -> Result<Vec<HvacMultispeedParameters>, DefaultsError> {
    const MAX_HVAC_STAGES: usize = 4;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let mut rdr = csv::Reader::from_path(path).map_err(|e| DefaultsError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
    })?;

    let mut rows = Vec::new();
    for rec in rdr.deserialize::<HashMap<String, String>>() {
        let record = rec.map_err(|e| DefaultsError::MalformedToml {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let hvac_name = record
            .get("HVAC Name")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if hvac_name.is_empty() {
            continue;
        }

        let speeds = record
            .get("Number of Speeds")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(1);

        let (efficiency_value, efficiency_kind) = parse_efficiency_cell(
            record
                .get("HVAC Efficiency")
                .map(String::as_str)
                .unwrap_or(""),
        );

        let stage_limit = speeds.clamp(1, MAX_HVAC_STAGES);
        let (capacity_ratios, airflow_ratios, cops, shrs) =
            parse_multispeed_stage_values(&record, stage_limit);

        if capacity_ratios.is_empty() || cops.is_empty() {
            continue;
        }

        rows.push(HvacMultispeedParameters {
            hvac_name,
            efficiency_kind,
            efficiency_value,
            number_of_speeds: speeds,
            capacity_ratios,
            airflow_ratios,
            cops,
            shrs,
        });
    }

    Ok(rows)
}

fn parse_multispeed_stage_values(
    record: &HashMap<String, String>,
    stage_limit: usize,
) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut capacity_ratios = Vec::new();
    let mut airflow_ratios = Vec::new();
    let mut cops = Vec::new();
    let mut shrs = Vec::new();

    for stage in 1..=stage_limit {
        let cap = parse_optional_f64(record.get(&format!("Capacity Ratio {stage}")));
        let flow = parse_optional_f64(record.get(&format!("Air Flow Ratio {stage}")));
        let cop = parse_optional_f64(record.get(&format!("COP {stage}")));
        let shr = parse_optional_f64(record.get(&format!("SHR {stage}")));

        if let (Some(cap), Some(flow), Some(cop)) = (cap, flow, cop) {
            capacity_ratios.push(cap);
            airflow_ratios.push(flow);
            cops.push(cop);
            if let Some(shr) = shr {
                shrs.push(shr);
            }
        }
    }

    (capacity_ratios, airflow_ratios, cops, shrs)
}

fn parse_optional_f64(raw: Option<&String>) -> Option<f64> {
    raw.and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            trimmed.parse::<f64>().ok()
        }
    })
}

fn parse_efficiency_cell(raw: &str) -> (f64, String) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (0.0, "SEER".to_string());
    }

    let mut parts = trimmed.split_whitespace();
    let value = parts
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0);
    let kind = parts.next().unwrap_or("SEER").to_ascii_uppercase();
    (value, kind)
}

fn normalize_equipment_key(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut last_was_sep = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            normalized.push('_');
            last_was_sep = true;
        }
    }

    let trimmed = normalized.trim_matches('_');
    if trimmed.is_empty() {
        raw.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_subdirs(root: &Path, names: &[&str]) {
        for name in names {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
    }

    fn write_minimal_zip(path: &Path) {
        std::fs::write(
            path.join("zip_parameters.toml"),
            r#"
[test_equipment]
zp = 1.0
ip = 0.0
pp = 0.0
zq = 1.0
iq = 0.0
pq = 0.0
pf = 1.0
"#,
        )
        .unwrap();
    }

    #[test]
    fn loads_zip_parameters_from_real_fixture() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let store = DefaultsStore::load(&defaults_dir).expect("load defaults");
        assert!(store.zip_count() > 0, "should load at least one ZIP entry");

        let ac = store
            .zip_params("Air Conditioner")
            .expect("air_conditioner ZIP");
        assert!((ac.pf - 0.96).abs() < 1e-10);
        assert!((ac.zp - 1.60).abs() < 1e-10);
        assert!((ac.pp - 2.09).abs() < 1e-10);
    }

    #[test]
    fn zip_params_returns_none_for_missing_equipment() {
        let store = DefaultsStore::empty();
        assert!(store.zip_params("nonexistent").is_none());
    }

    #[test]
    fn missing_zip_file_returns_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = DefaultsStore::load(dir.path()).unwrap_err();
        assert!(
            matches!(err, DefaultsError::MissingFile(_)),
            "expected MissingFile, got: {err:?}"
        );
    }

    #[test]
    fn malformed_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("zip_parameters.toml");
        std::fs::write(&zip_path, "this is { not valid toml").unwrap();
        let result = DefaultsStore::load(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, DefaultsError::MalformedToml { .. }),
            "expected MalformedToml, got: {err:?}"
        );
    }

    #[test]
    fn loads_hvac_curve_set_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());

        let hvac_dir = dir.path().join("hvac_cooling");
        std::fs::create_dir_all(&hvac_dir).unwrap();
        let mut f = std::fs::File::create(hvac_dir.join("air_conditioner.toml")).unwrap();
        write!(
            f,
            r#"
[[variant]]
name = "Single_1"
cap_t = [1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043]
cap_ff = [0.718605468, 0.41009989, -0.128705457]
eir_t = [-0.30428, 0.11805, -0.00342, -0.00626, 0.0007, -0.00047]
eir_ff = [1.32299905, -0.477711207, 0.154712157]
eir_plr = [0.93, 0.07, 0.0]
twb_bounds = [13.88, 23.88]
tdb_bounds = [18.33, 51.66]
"#
        )
        .unwrap();

        create_subdirs(
            dir.path(),
            &[
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let curves = store
            .hvac_cooling_coefficients("Air Conditioner")
            .expect("air_conditioner curves");
        assert_eq!(curves.variants.len(), 1);
        assert_eq!(curves.variants[0].name, "Single_1");

        let cap = &curves.variants[0].cap_t;
        assert!((cap.coeffs[0] - 1.5509).abs() < 1e-10);
        assert!((cap.x1_bounds.0 - 13.88).abs() < 1e-10);
    }

    #[test]
    fn loads_equipment_defaults_from_toml_subdir() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());

        let battery_dir = dir.path().join("battery");
        std::fs::create_dir_all(&battery_dir).unwrap();
        std::fs::write(
            battery_dir.join("default_parameters.toml"),
            r#"
capacity_kwh = 13.5
max_power_kw = 5.0
round_trip_efficiency = 0.9
"#,
        )
        .unwrap();

        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "envelope",
                "ev",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let batt = store
            .equipment_defaults(DefaultsCategory::Battery, "Default Parameters")
            .expect("battery defaults");
        let cap = batt.params.get("capacity_kwh").expect("capacity_kwh");
        assert_eq!(cap.as_float(), Some(13.5));
    }

    #[test]
    fn multispeed_lookup_loads_capacity_cop_and_shr_arrays() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        std::fs::write(
            dir.path().join("HVAC Multispeed Parameters.csv"),
            "HVAC Name,HVAC Efficiency,Number of Speeds,Capacity Ratio 1,Air Flow Ratio 1,COP 1,Capacity Ratio 2,Air Flow Ratio 2,COP 2,SHR 1,SHR 2\n\
ASHP Cooler,16.0 SEER,2,0.72,0.86,4.33748,1.0,1.0,3.99889,0.71597,0.72878\n",
        )
        .unwrap();

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let row = store
            .hvac_multispeed_parameters("ASHP Cooler", "SEER", 2, 16.0)
            .expect("multispeed row");
        assert_eq!(row.capacity_ratios, vec![0.72, 1.0]);
        assert_eq!(row.cops, vec![4.33748, 3.99889]);
        assert_eq!(row.shrs, vec![0.71597, 0.72878]);
    }

    #[test]
    fn can_load_one_entry_in_each_equipment_subdir() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());

        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        for category in [
            "battery",
            "envelope",
            "ev",
            "generator",
            "loads",
            "pv",
            "water_heating",
        ] {
            std::fs::write(
                dir.path().join(category).join("example.toml"),
                "example_value = 1.0\n",
            )
            .unwrap();
        }

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert!(store.category_count(DefaultsCategory::Battery) > 0);
        assert!(store.category_count(DefaultsCategory::Envelope) > 0);
        assert!(store.category_count(DefaultsCategory::Ev) > 0);
        assert!(store.category_count(DefaultsCategory::Generator) > 0);
        assert!(store.category_count(DefaultsCategory::Loads) > 0);
        assert!(store.category_count(DefaultsCategory::Pv) > 0);
        assert!(store.category_count(DefaultsCategory::WaterHeating) > 0);
    }

    #[test]
    fn empty_defaults_dirs_loads_successfully() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("zip_parameters.toml"), "").unwrap();
        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load empty defaults");
        assert_eq!(store.zip_count(), 0);
    }
}
