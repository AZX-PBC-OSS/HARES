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
//! - `ochre/defaults/Water Heating/*` -> `defaults/water_heating/*.toml` and `defaults/water_heating/default_paramters.csv`
//! - appliance and event schedule defaults -> `defaults/loads/*.toml`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hares_physics::biquadratic::BiquadraticCurve;
use hares_physics::constants::BTU_PER_HR_PER_W;
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
    pub ff_bounds: Option<(f64, f64)>,
    pub plf_bounds: Option<(f64, f64)>,
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

/// PV panel specification loaded from a TOML file in `defaults/pv/`.
///
/// Each TOML file defines one panel model's physical and performance
/// parameters. Fields map to the PV sizing module's override parameters
/// and to PVWatts v8 module type configuration.
///
/// Source citations:
/// - NOCT defaults from SAM PVWatts v8 (NREL/TP-7A40-80694 §2.4)
/// - Module type gammas from SAM PVWatts v8 SSC defaults
/// - System losses default 0.14 from EnergyPlus PVWatts V26-1-0 field N6
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PvPanelDefaults {
    /// Human-readable panel model name (e.g. "Standard 440W").
    pub name: String,
    /// STC nameplate wattage (W).
    pub panel_watts: u32,
    /// Module footprint area (m²).
    pub panel_area_m2: f64,
    /// Nominal Operating Cell Temperature (°C) per IEC 61215.
    /// PVWatts v8 default: 45°C for standard, 43°C for premium.
    #[serde(default = "PvPanelDefaults::default_noct_c")]
    pub noct_c: f64,
    /// Module type string matching PVWatts v8 enumeration:
    /// "standard", "premium", "thin_film".
    #[serde(default = "PvPanelDefaults::default_module_type")]
    pub module_type: String,
    /// Total system derate fraction (wiring, soiling, mismatch,
    /// inverter, shading). EnergyPlus PVWatts default = 0.14.
    #[serde(default = "PvPanelDefaults::default_system_losses")]
    pub system_losses_fraction: f64,
}

impl PvPanelDefaults {
    fn default_noct_c() -> f64 {
        45.0
    }
    fn default_module_type() -> String {
        "standard".to_string()
    }
    fn default_system_losses() -> f64 {
        0.14
    }
}

/// Generic default parameters loaded from a TOML file in any equipment
/// subdirectory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct EquipmentDefaults {
    pub params: HashMap<String, toml::Value>,
}

/// One row from `defaults/water_heating/default_paramters.csv`.
#[derive(Debug, Clone, PartialEq)]
struct WaterHeatingDefaultRow {
    description: String,
    name: String,
    value: f64,
    units: String,
}

/// Parsed water heater default parameters loaded from
/// `defaults/water_heating/default_paramters.csv`.
///
/// Provides typed lookups for tank volumes, UA values, UEF/EF efficiency
/// ratings, heating capacity, draw profiles, and setpoint schedules.
///
/// Source citations:
/// - Tank volumes: standard US residential sizes, ASHRAE HoF 2021 Ch.51
/// - UEF values: representative of typical installed residential stock
///   (pre-2015 vintage); these are not DOE 10 CFR Part 430, Subpart B, App. E
///   minimum-compliance baselines, which are higher for electric-resistance
///   units under the 2017 rule
/// - UA values: surface-area-derived from cylindrical tank geometry with
///   R-10/R-12 jacket insulation (insulation R-values per ASHRAE HoF 2021
///   Ch.26 Table 2) and a plumbing/fitting correction factor of ~1.15 applied
///   to the pure cylindrical area to account for pipe connections and
///   uninsulated tank top/bottom sections
/// - Draw profiles: DOE UEF test procedure medium draw bin (208.2 L/day);
///   low/high from ASHRAE HoF 2021 Ch.51 typical residential ranges
#[derive(Debug, Clone, Default)]
pub struct WaterHeatingDefaults {
    rows: Vec<WaterHeatingDefaultRow>,
}

impl WaterHeatingDefaults {
    /// Look up a numeric value by its CSV row name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<f64> {
        self.rows.iter().find(|r| r.name == name).map(|r| r.value)
    }

    /// Look up a numeric value by its CSV row name, returning `default` if absent.
    #[must_use]
    pub fn get_or(&self, name: &str, default: f64) -> f64 {
        self.get(name).unwrap_or(default)
    }

    /// Number of parameter rows loaded.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Iterator over all tank volume gallon sizes present in the defaults.
    pub fn tank_sizes_gal(&self) -> impl Iterator<Item = u8> + '_ {
        [30u8, 40, 50, 65, 80]
            .into_iter()
            .filter(|gal| self.get(&format!("Vol_{gal}gal_m3")).is_some())
    }

    /// Tank volume in m³ for a given gallon size.
    #[must_use]
    pub fn tank_volume_m3(&self, gallons: u8) -> Option<f64> {
        self.get(&format!("Vol_{gallons}gal_m3"))
    }

    /// Tank height in m for a given gallon size.
    #[must_use]
    pub fn tank_height_m(&self, gallons: u8) -> Option<f64> {
        self.get(&format!("H_{gallons}gal"))
    }

    /// UA heat loss coefficient in W/K for a given gallon size with R-12 insulation.
    #[must_use]
    pub fn ua_r12_w_per_k(&self, gallons: u8) -> Option<f64> {
        self.get(&format!("UA_{gallons}gal_R12"))
    }

    /// UA heat loss coefficient in W/K for a given gallon size with R-10 insulation.
    #[must_use]
    pub fn ua_r10_w_per_k(&self, gallons: u8) -> Option<f64> {
        self.get(&format!("UA_{gallons}gal_R10"))
    }

    /// Look up a UEF value by row name (e.g. "UEF_GasStorage_50gal").
    #[must_use]
    pub fn uef(&self, name: &str) -> Option<f64> {
        self.get(name)
    }

    /// Look up heating capacity in W by row name.
    #[must_use]
    pub fn heating_capacity_w(&self, name: &str) -> Option<f64> {
        self.get(name)
    }
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
    /// Typed PV panel specifications loaded from `defaults/pv/*.toml`.
    pv_panel: HashMap<String, PvPanelDefaults>,
    water_heating: HashMap<String, EquipmentDefaults>,
    /// Typed water heater defaults loaded from `defaults/water_heating/default_paramters.csv`.
    water_heating_csv: Option<WaterHeatingDefaults>,
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
        store.pv_panel = load_pv_panel_defaults(&defaults_dir.join("pv"))?;
        store.water_heating = load_toml_dir(&defaults_dir.join("water_heating"))?;
        store.water_heating_csv = load_water_heating_csv(
            &defaults_dir
                .join("water_heating")
                .join("default_paramters.csv"),
        );
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if let Some(ref wh) = store.water_heating_csv {
                check_water_heating_invariants(wh);
            }
        }

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

    /// Look up a PV panel specification by file-stem name (canonicalized
    /// to lowercase snake case).
    #[must_use]
    pub fn pv_panel_defaults(&self, name: &str) -> Option<&PvPanelDefaults> {
        self.pv_panel.get(&normalize_equipment_key(name))
    }

    /// Number of PV panel specifications loaded.
    #[must_use]
    pub fn pv_panel_count(&self) -> usize {
        self.pv_panel.len()
    }

    /// Take ownership of the loaded PV panel defaults map, leaving an empty
    /// map in its place. Called by `Dwelling::from_preparsed` to transfer
    /// panel defaults to the Dwelling struct so they are accessible at PV
    /// sizing call sites.
    pub fn take_pv_panel_map(&mut self) -> HashMap<String, PvPanelDefaults> {
        std::mem::take(&mut self.pv_panel)
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

    /// Access typed water heater defaults loaded from
    /// `defaults/water_heating/default_paramters.csv`.
    #[must_use]
    pub fn water_heating_defaults(&self) -> Option<&WaterHeatingDefaults> {
        self.water_heating_csv.as_ref()
    }

    /// Whether the water heater CSV defaults were loaded successfully.
    #[must_use]
    pub fn has_water_heating_defaults(&self) -> bool {
        self.water_heating_csv.is_some()
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

/// Load typed PV panel specification TOML files from `defaults/pv/`.
///
/// Each `.toml` file is deserialized into a [`PvPanelDefaults`] keyed by
/// normalized file stem name. Errors on missing files silently produce an
/// empty map — the caller is responsible for falling back to compile-time
/// constants.
fn load_pv_panel_defaults(dir: &Path) -> Result<HashMap<String, PvPanelDefaults>, DefaultsError> {
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
            let defaults: PvPanelDefaults = match toml::from_str(&content) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "skipping non-conforming TOML in defaults/pv/ — not a PvPanelDefaults file"
                    );
                    continue;
                }
            };
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
                warn_on_clamp: true,
                output_min: Some(0.0),
                output_max: None,
            },
            cap_ff: v.cap_ff,
            eir_t: BiquadraticCurve {
                coeffs: v.eir_t,
                x1_bounds: (v.twb_bounds[0], v.twb_bounds[1]),
                x2_bounds: (v.tdb_bounds[0], v.tdb_bounds[1]),
                warn_on_clamp: true,
                output_min: None,
                output_max: None,
            },
            eir_ff: v.eir_ff,
            eir_plr: v.eir_plr,
            ff_bounds: None,
            plf_bounds: None,
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
    let get_row_with_default = |name: &str, default: f64| -> Vec<f64> {
        data.get(name)
            .cloned()
            .unwrap_or_else(|| vec![default; n_variants])
    };
    let ff_min_row = data.get("min_ff").cloned();
    let ff_max_row = data.get("max_ff").cloned();
    let plf_min_row = data.get("min_plf").cloned();
    let plf_max_row = data.get("max_plf").cloned();

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
            let twb_min = get_row_with_default("min_Twb", -10.0)[i];
            let twb_max = get_row_with_default("max_Twb", 50.0)[i];
            let tdb_min = get_row_with_default("min_Tdb", -50.0)[i];
            let tdb_max = get_row_with_default("max_Tdb", 60.0)[i];

            HvacCurveVariant {
                name: variant_names[i].clone(),
                cap_t: BiquadraticCurve {
                    coeffs: cap_t,
                    x1_bounds: (twb_min, twb_max),
                    x2_bounds: (tdb_min, tdb_max),
                    warn_on_clamp: true,
                    output_min: Some(0.0),
                    output_max: None,
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
                    warn_on_clamp: true,
                    output_min: None,
                    output_max: None,
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
                ff_bounds: match (&ff_min_row, &ff_max_row) {
                    (Some(mins), Some(maxs)) => Some((mins[i], maxs[i])),
                    _ => None,
                },
                plf_bounds: match (&plf_min_row, &plf_max_row) {
                    (Some(mins), Some(maxs)) => Some((mins[i], maxs[i])),
                    _ => None,
                },
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

        // Validate that the rated-speed COP is not anomalously low relative to the
        // SEER/EER-derived expectation.  HSPF→COP involves regional climate factors
        // and AFUE is a percentage, so this check is restricted to SEER and EER
        // entries where COP ≈ efficiency_value / BTU_PER_HR_PER_W.
        // EnergyPlus StandardRatings.hh:71 defines ConvFromSIToIP = 3.412141633.
        if efficiency_kind == "SEER" || efficiency_kind == "EER" {
            if let Some(&last_cop) = cops.last() {
                let expected = efficiency_value / BTU_PER_HR_PER_W;
                let deviation = (last_cop - expected).abs() / expected;
                if deviation > 0.30 {
                    tracing::warn!(
                        hvac_name = %hvac_name,
                        %efficiency_kind,
                        efficiency_value,
                        last_cop,
                        expected_cop = expected,
                        deviation_pct = deviation * 100.0,
                        "CSV row rated-speed COP deviates from SEER/EER-derived \
                         expectation by {:.1}% (>30%); values may be erroneous",
                        deviation * 100.0,
                    );
                }
            }
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

    // Invariant: for equipment rows with the same name, speed count, and
    // efficiency kind, COP at each speed should increase with the efficiency
    // rating. A reversal indicates a data entry error where COPs from a
    // lower-efficiency unit were copied to a higher-rated row. The check
    // uses a 1% tolerance for floating-point rounding, comparing against the
    // proportional COP expected from the HSPF ratio.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        check_hspf_cop_monotonicity(&rows);
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

#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_hspf_cop_monotonicity(rows: &[HvacMultispeedParameters]) {
    // Group rows by (hvac_name, number_of_speeds, efficiency_kind) and
    // validate that COPs at each speed are monotonically increasing with
    // the efficiency rating. A COP that is lower for a higher-rated unit
    // than for a lower-rated unit signals a data entry error.
    use std::collections::HashMap;

    let mut groups: HashMap<(String, usize, String), Vec<&HvacMultispeedParameters>> =
        HashMap::new();
    for row in rows {
        let key = (
            normalize_equipment_key(&row.hvac_name),
            row.number_of_speeds,
            row.efficiency_kind.clone(),
        );
        groups.entry(key).or_default().push(row);
    }

    for group in groups.values() {
        if group.len() < 2 {
            continue;
        }
        // Sort by efficiency rating ascending.
        let mut sorted: Vec<&&HvacMultispeedParameters> = group.iter().collect();
        sorted.sort_by(|a, b| {
            a.efficiency_value
                .partial_cmp(&b.efficiency_value)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let n_stages = sorted[0].cops.len();
        for stage in 0..n_stages {
            for i in 1..sorted.len() {
                let cop_prev = sorted[i - 1].cops[stage];
                let cop_curr = sorted[i].cops[stage];
                let eff_prev = sorted[i - 1].efficiency_value;
                let eff_curr = sorted[i].efficiency_value;

                // Compare against proportional COP expected from the HSPF ratio.
                // 1% tolerance accounts for floating-point rounding only.
                let expected_cop = cop_prev * (eff_curr / eff_prev);
                if cop_curr < expected_cop * 0.99 {
                    let stage_num = stage + 1;
                    tracing::warn!(
                        hvac_name = %sorted[i].hvac_name,
                        prev_hvac_name = %sorted[i - 1].hvac_name,
                        eff_kind = %sorted[i].efficiency_kind,
                        prev_eff = eff_prev,
                        curr_eff = eff_curr,
                        stage = stage_num,
                        prev_cop = cop_prev,
                        curr_cop = cop_curr,
                        expected_cop,
                        n_speeds = sorted[i].number_of_speeds,
                        "COP at speed {stage_num} is {cop_curr:.3} but {eff_prev:.2}-rated \
                         unit has COP {cop_prev:.3} at same speed; expected ~{expected_cop:.3} \
                         from proportional scaling of the higher rating"
                    );
                }
            }
        }
    }
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

/// Load water heater default parameters from the CSV file at `path`.
///
/// The CSV has columns: Description, Name, Value, Units.
/// Rows whose Value field is empty or unparseable are skipped with a warning.
/// Returns `None` if the file does not exist (non-fatal — callers fall back).
fn load_water_heating_csv(path: &Path) -> Option<WaterHeatingDefaults> {
    if !path.exists() {
        return None;
    }
    let mut rdr = match csv::Reader::from_path(path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to open water heating defaults CSV"
            );
            return None;
        }
    };
    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping malformed row in water heating defaults CSV"
                );
                continue;
            }
        };
        let description = record.get(0).unwrap_or("").to_string();
        let name = record.get(1).unwrap_or("").to_string();
        let value_str = record.get(2).unwrap_or("");
        let units = record.get(3).unwrap_or("").to_string();

        if name.is_empty() || value_str.trim().is_empty() {
            continue;
        }

        match value_str.trim().parse::<f64>() {
            Ok(v) if v.is_finite() => {
                rows.push(WaterHeatingDefaultRow {
                    description,
                    name,
                    value: v,
                    units,
                });
            }
            Ok(_) => {
                tracing::warn!(
                    name = %name,
                    value = %value_str.trim(),
                    "non-finite value in water heating defaults CSV"
                );
            }
            Err(e) => {
                tracing::warn!(
                    name = %name,
                    value = %value_str.trim(),
                    error = %e,
                    "unparseable value in water heating defaults CSV"
                );
            }
        }
    }
    Some(WaterHeatingDefaults { rows })
}

/// Invariant checks for water heater defaults loaded at startup.
///
/// Validates:
/// - UA values are within the physically plausible range (0.5–5.0 W/K) for residential tanks
/// - UA values increase monotonically with tank volume (larger tanks have higher heat loss)
/// - UEF values are within valid ranges (0.0–1.0 for electric/gas, 1.0–5.0 for HPWH)
/// - Tank volumes are in standard sizes with correct gallon-to-liter conversions
/// - Every required field has a corresponding entry
///
/// Core invariants that would corrupt simulation results produce `tracing::error!`;
/// minor anomalies produce `tracing::warn!`.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_water_heating_invariants(wh: &WaterHeatingDefaults) {
    // UA monotonicity: larger tanks must have higher UA (more surface area).
    let sizes: [u8; 5] = [30, 40, 50, 65, 80];
    let mut prev_ua: Option<f64> = None;
    for &gal in &sizes {
        if let Some(ua) = wh.ua_r12_w_per_k(gal) {
            // UA must be within physically plausible range for residential tanks.
            if !(0.5..=5.0).contains(&ua) {
                tracing::error!(
                    gal,
                    ua_w_per_k = ua,
                    "UA for {gal}-gal tank ({ua:.2} W/K) outside plausible range [0.5, 5.0] W/K"
                );
            }
            if let Some(prev) = prev_ua {
                if ua <= prev {
                    tracing::error!(
                        gal,
                        prev_ua = prev,
                        ua_w_per_k = ua,
                        "UA for {gal}-gal tank ({ua:.2} W/K) not greater than \
                         previous size ({prev:.2} W/K); UA must increase monotonically with tank volume"
                    );
                }
            }
            prev_ua = Some(ua);
        }
    }

    // UEF range checks.
    // Gas storage and electric resistance: valid range [0.0, 1.0].
    // HPWH: valid range [1.0, 5.0] (UEF can exceed 1.0 because the heat pump
    // moves more heat than the electrical energy it consumes).
    for (uef_name, min, max) in [
        ("UEF_GasStorage_30gal", 0.0, 1.0),
        ("UEF_GasStorage_40gal", 0.0, 1.0),
        ("UEF_GasStorage_50gal", 0.0, 1.0),
        ("UEF_GasStorage_65gal", 0.0, 1.0),
        ("UEF_GasStorage_80gal", 0.0, 1.0),
        ("UEF_ElecRes_30gal", 0.0, 1.0),
        ("UEF_ElecRes_40gal", 0.0, 1.0),
        ("UEF_ElecRes_50gal", 0.0, 1.0),
        ("UEF_ElecRes_65gal", 0.0, 1.0),
        ("UEF_ElecRes_80gal", 0.0, 1.0),
        ("UEF_HPWH_50gal", 1.0, 5.0),
        ("UEF_HPWH_65gal", 1.0, 5.0),
        ("UEF_HPWH_80gal", 1.0, 5.0),
    ] {
        if let Some(uef) = wh.uef(uef_name) {
            if !(min..=max).contains(&uef) {
                tracing::error!(
                    name = %uef_name,
                    uef_value = uef,
                    valid_range = format!("[{min}, {max}]"),
                    "UEF value {uef} for {uef_name} outside valid range [{min}, {max}]"
                );
            }
        }
    }

    // Tank volume consistency: check gallon → liter → m³ conversions.
    for gal in sizes {
        let key_m3 = format!("Vol_{gal}gal_m3");
        let key_l = format!("Vol_{gal}gal_L");
        if let (Some(m3), Some(l)) = (wh.get(&key_m3), wh.get(&key_l)) {
            let expected_l = m3 * 1000.0;
            let pct_error = ((l - expected_l) / expected_l).abs() * 100.0;
            if pct_error > 1.0 {
                tracing::error!(
                    gal,
                    vol_m3 = m3,
                    vol_l = l,
                    expected_l = expected_l,
                    pct_error,
                    "{gal}-gal tank: m³→L conversion error {pct_error:.2}% (m³={m3:.4} → {expected_l:.1} L, got {l:.1} L)"
                );
            }
        }
    }

    // Required field coverage: every parameter field that the defaults CSV is
    // expected to supply must have a corresponding entry. Missing entries mean
    // downstream code silently receives None for that field.
    let required: &[&str] = &[
        // Per-size tank volumes and heights
        "Vol_30gal_m3",
        "Vol_30gal_L",
        "Vol_40gal_m3",
        "Vol_40gal_L",
        "Vol_50gal_m3",
        "Vol_50gal_L",
        "Vol_65gal_m3",
        "Vol_65gal_L",
        "Vol_80gal_m3",
        "Vol_80gal_L",
        "H_30gal",
        "H_40gal",
        "H_50gal",
        "H_65gal",
        "H_80gal",
        // Per-size UA values (R-12 and R-10)
        "UA_30gal_R12",
        "UA_40gal_R12",
        "UA_50gal_R12",
        "UA_65gal_R12",
        "UA_80gal_R12",
        "UA_30gal_R10",
        "UA_40gal_R10",
        "UA_50gal_R10",
        "UA_65gal_R10",
        "UA_80gal_R10",
        // UEF values by fuel type and tank size
        "UEF_GasStorage_30gal",
        "UEF_GasStorage_40gal",
        "UEF_GasStorage_50gal",
        "UEF_GasStorage_65gal",
        "UEF_GasStorage_80gal",
        "UEF_ElecRes_30gal",
        "UEF_ElecRes_40gal",
        "UEF_ElecRes_50gal",
        "UEF_ElecRes_65gal",
        "UEF_ElecRes_80gal",
        "UEF_HPWH_50gal",
        "UEF_HPWH_65gal",
        "UEF_HPWH_80gal",
        "UEF_TanklessGas",
        // Legacy EF values
        "EF_GasStorage_50gal",
        "EF_ElecRes_50gal",
        // Heating capacities by fuel type and tank size
        "HC_GasStorage_30gal",
        "HC_GasStorage_40gal",
        "HC_GasStorage_50gal",
        "HC_GasStorage_65gal",
        "HC_GasStorage_80gal",
        "HC_ElecRes",
        "HC_TanklessGas",
        // Conversion efficiency
        "ConvEff_GasStorage",
        // Insulation R-values
        "R_val_R10",
        "R_val_R12",
        "R_val_R16",
        // Common temperature parameters
        "T_set",
        "T_db",
        "T_max",
        "T_init",
        // Draw profiles
        "Draw_Low",
        "Draw_Medium",
        "Draw_High",
        "Draw_Flow",
        // Tank nodes and element powers
        "N_tank",
        "P_hw1",
        "P_hw2",
    ];
    for &name in required {
        if wh.get(name).is_none() {
            tracing::error!(
                name,
                "required water heating defaults CSV field '{name}' is missing"
            );
        }
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

        assert!(
            store.pv_panel_count() >= 2,
            "should load at least two PV panel specs"
        );
        let std440 = store
            .pv_panel_defaults("standard_440")
            .expect("standard_440 PV panel spec");
        assert_eq!(std440.panel_watts, 440);
        assert!((std440.panel_area_m2 - 2.1).abs() < f64::EPSILON);
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
    fn hvac_csv_missing_temperature_bounds_uses_tightened_fallbacks_and_reads_plf_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("mshp.csv");
        std::fs::write(
            &csv_path,
            "Name,Variable_1\n\
a_eir_t,0.1\n\
b_eir_t,0.0\n\
c_eir_t,0.0\n\
d_eir_t,0.0\n\
e_eir_t,0.0\n\
f_eir_t,0.0\n\
a_eir_ff,1.0\n\
b_eir_ff,0.0\n\
c_eir_ff,0.0\n\
a_eir_plr,1.0\n\
b_eir_plr,0.0\n\
c_eir_plr,0.0\n\
a_cap_t,1.0\n\
b_cap_t,0.0\n\
c_cap_t,0.0\n\
d_cap_t,0.0\n\
e_cap_t,0.0\n\
f_cap_t,0.0\n\
a_cap_ff,1.0\n\
b_cap_ff,0.0\n\
c_cap_ff,0.0\n\
min_plf,0.48\n\
max_plf,1.0\n",
        )
        .unwrap();

        let set = load_hvac_csv_file(&csv_path).expect("csv should parse");
        let v = &set.variants[0];
        assert_eq!(v.cap_t.x1_bounds, (-10.0, 50.0));
        assert_eq!(v.cap_t.x2_bounds, (-50.0, 60.0));
        assert_eq!(v.plf_bounds, Some((0.48, 1.0)));
        assert_eq!(v.ff_bounds, None);
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

    #[test]
    fn load_pv_panel_defaults_deserializes_complete_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());

        let pv_dir = dir.path().join("pv");
        std::fs::create_dir_all(&pv_dir).unwrap();
        std::fs::write(
            pv_dir.join("standard_440.toml"),
            r#"
name = "Standard 440W"
panel_watts = 440
panel_area_m2 = 2.1
noct_c = 45.0
module_type = "standard"
system_losses_fraction = 0.14
"#,
        )
        .unwrap();

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
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert_eq!(store.pv_panel_count(), 1);

        let panel = store
            .pv_panel_defaults("standard_440")
            .expect("standard_440 panel");
        assert_eq!(panel.name, "Standard 440W");
        assert_eq!(panel.panel_watts, 440);
        assert!((panel.panel_area_m2 - 2.1).abs() < 1e-10);
        assert!((panel.noct_c - 45.0).abs() < 1e-10);
        assert_eq!(panel.module_type, "standard");
        assert!((panel.system_losses_fraction - 0.14).abs() < 1e-10);
    }

    #[test]
    fn load_pv_panel_defaults_falls_back_when_dir_empty() {
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

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        // No .toml files in pv/, so pv_panel_count should be 0.
        // The caller should fall back to compile-time constants.
        assert_eq!(store.pv_panel_count(), 0);
        assert!(store.pv_panel_defaults("anything").is_none());
    }

    #[test]
    fn ashp_heater_4speed_cops_monotonically_increase_with_hspf() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let store = DefaultsStore::load(&defaults_dir).expect("load defaults");

        // Collect all ASHP Heater 4-speed HSPF rows sorted by efficiency.
        let mut ashp_heater_4sp: Vec<&HvacMultispeedParameters> = store
            .hvac_multispeed
            .iter()
            .filter(|r| {
                normalize_equipment_key(&r.hvac_name) == "ashp_heater"
                    && r.number_of_speeds == 4
                    && r.efficiency_kind == "HSPF"
            })
            .collect();
        ashp_heater_4sp.sort_by(|a, b| {
            a.efficiency_value
                .partial_cmp(&b.efficiency_value)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        assert!(
            ashp_heater_4sp.len() >= 3,
            "expected at least 3 ASHP Heater 4-speed HSPF rows, got {}",
            ashp_heater_4sp.len()
        );

        let n_stages = ashp_heater_4sp[0].cops.len();
        for stage in 0..n_stages {
            for i in 1..ashp_heater_4sp.len() {
                let cop_prev = ashp_heater_4sp[i - 1].cops[stage];
                let cop_curr = ashp_heater_4sp[i].cops[stage];
                let eff_prev = ashp_heater_4sp[i - 1].efficiency_value;
                let eff_curr = ashp_heater_4sp[i].efficiency_value;
                let expected_cop = cop_prev * (eff_curr / eff_prev);
                assert!(
                    cop_curr > expected_cop * 0.99,
                    "ASHP Heater COP at speed {} not monotonic: {} HSPF COP={:.3} < {} HSPF COP={:.3} (expected ~{:.3} proportional)",
                    stage + 1,
                    eff_curr,
                    cop_curr,
                    eff_prev,
                    cop_prev,
                    expected_cop
                );
            }
        }
    }

    #[test]
    fn ashp_heater_11hspf_cop_lookup_returns_correct_values() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let store = DefaultsStore::load(&defaults_dir).expect("load defaults");

        let row = store
            .hvac_multispeed_parameters("ASHP Heater", "HSPF", 4, 11.0)
            .expect("11.0 HSPF ASHP Heater 4-speed row should exist");

        // Scaled from 10.47 HSPF BEopt row COPs [6.0975, 5.2085, 4.4020, 4.2058]
        // with scale factor 11.0/10.47 ≈ 1.0506 → [6.41, 5.47, 4.62, 4.42]
        assert!(
            (row.cops[0] - 6.41).abs() < 0.02,
            "COP₁ expected ~6.41, got {}",
            row.cops[0]
        );
        assert!(
            (row.cops[1] - 5.47).abs() < 0.02,
            "COP₂ expected ~5.47, got {}",
            row.cops[1]
        );
        assert!(
            (row.cops[2] - 4.62).abs() < 0.02,
            "COP₃ expected ~4.62, got {}",
            row.cops[2]
        );
        assert!(
            (row.cops[3] - 4.42).abs() < 0.02,
            "COP₄ expected ~4.42, got {}",
            row.cops[3]
        );
    }

    // ── Water heater defaults CSV tests ──────────────────────────────────

    /// Write a minimal valid water heating defaults CSV.
    fn write_water_heating_csv(dir: &Path) {
        std::fs::write(
            dir.join("default_paramters.csv"),
            r#"Description,Name,Value,Units
Tank_Volume_50gal_m3,Vol_50gal_m3,0.189,m3
Tank_Volume_50gal_L,Vol_50gal_L,189.3,L
Tank_Volume_80gal_m3,Vol_80gal_m3,0.303,m3
Tank_Volume_80gal_L,Vol_80gal_L,302.8,L
Tank_Height_50gal,H_50gal,1.22,m
UA_50gal_R12,UA_50gal_R12,1.10,W_per_K
UA_80gal_R12,UA_80gal_R12,1.49,W_per_K
UEF_GasStorage_50gal,UEF_GasStorage_50gal,0.64,
UEF_ElectricResistance_50gal,UEF_ElecRes_50gal,0.93,
UEF_HPWH_50gal,UEF_HPWH_50gal,3.70,
HeatingCapacity_GasStorage_50gal,HC_GasStorage_50gal,11170,W
Setpoint,T_set,51.67,C
Deadband,T_db,5.56,C
Avg_Draw_Medium_L_per_day,Draw_Medium,208.2,L_per_day
Draw_Flow_Rate_kg_s,Draw_Flow,0.1262,kg_per_s
"#,
        )
        .unwrap();
    }

    #[test]
    fn water_heating_csv_parses_successfully() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        write_water_heating_csv(&wh_dir);
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
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert!(store.has_water_heating_defaults());

        let wh = store.water_heating_defaults().unwrap();
        assert!(wh.row_count() > 10, "should have many parameter rows");
    }

    #[test]
    fn water_heating_csv_lookup_returns_correct_values() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        write_water_heating_csv(&wh_dir);
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
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let wh = store.water_heating_defaults().unwrap();

        // Tank volumes
        assert!(
            (wh.get("Vol_50gal_m3").unwrap() - 0.189).abs() < 1e-6,
            "50-gal volume in m³"
        );
        assert!(
            (wh.get("Vol_50gal_L").unwrap() - 189.3).abs() < 0.1,
            "50-gal volume in L"
        );

        // UA values
        assert!(
            (wh.get("UA_50gal_R12").unwrap() - 1.10).abs() < 1e-6,
            "50-gal UA with R-12"
        );
        assert!(
            (wh.get("UA_80gal_R12").unwrap() - 1.49).abs() < 1e-6,
            "80-gal UA with R-12"
        );

        // UEF values
        assert!(
            (wh.uef("UEF_GasStorage_50gal").unwrap() - 0.64).abs() < 1e-6,
            "gas storage UEF"
        );
        assert!(
            (wh.uef("UEF_ElecRes_50gal").unwrap() - 0.93).abs() < 1e-6,
            "electric resistance UEF"
        );
        assert!(
            (wh.uef("UEF_HPWH_50gal").unwrap() - 3.70).abs() < 1e-6,
            "HPWH UEF"
        );

        // Common parameters
        assert!((wh.get("T_set").unwrap() - 51.67).abs() < 1e-6, "setpoint");
        assert!((wh.get("T_db").unwrap() - 5.56).abs() < 1e-6, "deadband");
    }

    #[test]
    fn ua_increases_monotonically_with_tank_volume() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        write_water_heating_csv(&wh_dir);
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
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let wh = store.water_heating_defaults().unwrap();

        // UA should increase monotonically with tank volume.
        let ua_50 = wh.ua_r12_w_per_k(50).expect("UA_50gal_R12");
        let ua_80 = wh.ua_r12_w_per_k(80).expect("UA_80gal_R12");
        assert!(
            ua_80 > ua_50,
            "80-gal UA ({ua_80}) must exceed 50-gal UA ({ua_50}): larger tanks have more surface area"
        );
    }

    #[test]
    fn uef_values_are_within_valid_ranges() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        write_water_heating_csv(&wh_dir);
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
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let wh = store.water_heating_defaults().unwrap();

        // Gas/electric UEF must be 0.0–1.0 (storage tank efficiency).
        let gas_uef = wh.uef("UEF_GasStorage_50gal").expect("gas UEF");
        assert!(
            (0.0..=1.0).contains(&gas_uef),
            "gas storage UEF {gas_uef} outside [0.0, 1.0]"
        );

        let elec_uef = wh.uef("UEF_ElecRes_50gal").expect("elec UEF");
        assert!(
            (0.0..=1.0).contains(&elec_uef),
            "electric resistance UEF {elec_uef} outside [0.0, 1.0]"
        );

        // HPWH UEF must be 1.0–5.0 (heat pump can exceed 1.0).
        let hpwh_uef = wh.uef("UEF_HPWH_50gal").expect("HPWH UEF");
        assert!(
            (1.0..=5.0).contains(&hpwh_uef),
            "HPWH UEF {hpwh_uef} outside [1.0, 5.0]"
        );
    }

    #[test]
    fn water_heating_csv_regression_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        write_water_heating_csv(&wh_dir);
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
            ],
        );

        // Loading should succeed without panicking.
        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let wh = store.water_heating_defaults().unwrap();

        // All typed getters should return Some or None, never panic.
        let _ = wh.tank_volume_m3(50);
        let _ = wh.tank_volume_m3(30); // not in fixture, should be None
        let _ = wh.tank_height_m(50);
        let _ = wh.ua_r12_w_per_k(50);
        let _ = wh.ua_r10_w_per_k(50);
        let _ = wh.get("T_set");
        let _ = wh.get("T_db");
        let _ = wh.get("Draw_Medium");
        let _ = wh.get("Draw_Flow");
        let _ = wh.get_or("nonexistent", 42.0);

        // tank_sizes_gal should iterate only over sizes with volume data.
        let sizes: Vec<u8> = wh.tank_sizes_gal().collect();
        assert!(!sizes.is_empty(), "should have at least one tank size");
    }

    #[test]
    fn water_heating_csv_missing_file_is_non_fatal() {
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

        // No default_paramters.csv in water_heating dir — should load fine.
        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert!(
            !store.has_water_heating_defaults(),
            "no CSV → no water heating defaults"
        );
        assert!(store.water_heating_defaults().is_none());
    }

    #[test]
    fn water_heating_csv_malformed_rows_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        std::fs::write(
            wh_dir.join("default_paramters.csv"),
            r#"Description,Name,Value,Units
Good_Row,GoodKey,42.0,units
Bad_Row,BadKey,not_a_number,units
Empty_Value,EmptyVal,,units
Another_Good,Key2,3.14,m
"#,
        )
        .unwrap();
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
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let wh = store.water_heating_defaults().unwrap();

        assert!(
            (wh.get("GoodKey").unwrap() - 42.0).abs() < 1e-6,
            "good row should parse"
        );
        assert!(
            (wh.get("Key2").unwrap() - 3.14).abs() < 1e-6,
            "another good row should parse"
        );
        assert!(
            wh.get("BadKey").is_none(),
            "unparseable row should be absent"
        );
        assert!(
            wh.get("EmptyVal").is_none(),
            "empty-value row should be absent"
        );
    }

    #[test]
    fn partial_csv_does_not_panic_and_missing_fields_return_none() {
        // Regression: the invariant checker should catch missing required
        // fields without panicking. A CSV with only a few rows exercises the
        // partial-load path.
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        std::fs::write(
            wh_dir.join("default_paramters.csv"),
            r#"Description,Name,Value,Units
Tank_Volume_50gal_m3,Vol_50gal_m3,0.189,m3
Setpoint,T_set,60.0,C
"#,
        )
        .unwrap();
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
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let wh = store.water_heating_defaults().unwrap();

        // Present fields are accessible.
        assert!((wh.get("Vol_50gal_m3").unwrap() - 0.189).abs() < 1e-6);
        assert!((wh.get("T_set").unwrap() - 60.0).abs() < 1e-6);

        // Absent fields return None — the loader must not fabricate values.
        assert!(wh.get("T_db").is_none(), "T_db not in CSV → None");
        assert!(wh.get("UA_50gal_R12").is_none(), "UA not in CSV → None");
        assert!(wh.get("Draw_Medium").is_none(), "Draw not in CSV → None");
        assert!(wh.get("N_tank").is_none(), "N_tank not in CSV → None");
    }

    #[test]
    fn real_water_heating_csv_loads_from_fixture() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let store = DefaultsStore::load(&defaults_dir).expect("load defaults");

        assert!(
            store.has_water_heating_defaults(),
            "real defaults/water_heating/default_paramters.csv should be present and parseable"
        );

        let wh = store.water_heating_defaults().unwrap();

        // Verify key parameters from the real CSV are loaded.
        assert!(
            wh.row_count() > 30,
            "should have many rows; got {}",
            wh.row_count()
        );

        // Tank volumes for standard sizes.
        for gal in [30u8, 40, 50, 65, 80] {
            let m3 = wh.tank_volume_m3(gal);
            assert!(m3.is_some(), "{gal}-gal tank volume should exist");
            assert!(m3.unwrap() > 0.0, "{gal}-gal volume should be positive");
        }

        // UEF values for gas, electric, HPWH.
        assert!(
            wh.uef("UEF_GasStorage_50gal").is_some(),
            "gas storage UEF for 50gal"
        );
        assert!(
            wh.uef("UEF_ElecRes_50gal").is_some(),
            "electric resistance UEF for 50gal"
        );
        assert!(wh.uef("UEF_HPWH_50gal").is_some(), "HPWH UEF for 50gal");

        // Common parameters.
        assert!(wh.get("T_set").is_some(), "setpoint");
        assert!(wh.get("T_db").is_some(), "deadband");
        assert!(wh.get("T_max").is_some(), "max tank temp");
        assert!(wh.get("N_tank").is_some(), "tank nodes");
        assert!(wh.get("Draw_Medium").is_some(), "medium draw profile");
        assert!(wh.get("Draw_Flow").is_some(), "draw flow rate");

        // UA consistency: monotonic increase with tank size.
        let mut prev_ua = None;
        for gal in [30u8, 40, 50, 65, 80] {
            if let Some(ua) = wh.ua_r12_w_per_k(gal) {
                if let Some(prev) = prev_ua {
                    assert!(
                        ua > prev,
                        "UA must increase monotonically: {gal}-gal UA={ua:.3} ≤ previous UA={prev:.3}"
                    );
                }
                prev_ua = Some(ua);
            }
        }

        // UEF values within valid ranges.
        for (name, min, max) in [
            ("UEF_GasStorage_50gal", 0.0, 1.0_f64),
            ("UEF_ElecRes_50gal", 0.0, 1.0),
            ("UEF_HPWH_50gal", 1.0, 5.0),
        ] {
            if let Some(uef) = wh.uef(name) {
                assert!(
                    (min..=max).contains(&uef),
                    "{name} UEF={uef} outside [{min}, {max}]"
                );
            }
        }
    }
}
