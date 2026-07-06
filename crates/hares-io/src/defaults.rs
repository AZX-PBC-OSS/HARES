//! Default parameter loading from the `defaults/` directory tree.
//!
//! The [`DefaultsStore`] loads equipment ZIP parameters and HVAC biquadratic
//! coefficient sets at startup so that equipment specs can be fully resolved
//! before simulation begins.
//!
//! OCHRE defaults mapping (source -> HARES entry):
//! - `ochre/defaults/ZIP Parameters.csv` -> `defaults/zip_parameters.toml` -> [`ZipLoad`]
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
use hares_physics::units::power_kw_to_w;
/// ZIP load model parameters for voltage-dependent power modelling.
///
/// Re-exported from the shared contract crate: `defaults/zip_parameters.toml`
/// rows deserialize directly into [`ZipLoad`] (field names `zp`/`ip`/`pp`/
/// `zq`/`iq`/`pq`/`pf` match; `v0` defaults to 1.0). The toml and the in-code
/// class table [`hares_types::zip::zip_defaults_for_class`] are dual
/// representations of the same data — a drift test in this module keeps them
/// in exact agreement.
pub use hares_types::zip::ZipLoad;
use serde::Deserialize;
use thiserror::Error;

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
    /// Marks this spec as the panel used when the caller provides no
    /// explicit panel parameters. Exactly one spec in `defaults/pv/`
    /// should set `default = true`; consumers fall back to the
    /// lexicographically-first key when none is marked, so adding a new
    /// panel spec never silently changes the default selection.
    #[serde(default)]
    pub default: bool,
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

/// One point in the generator efficiency curve (TOML deserialization only).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GeneratorCurvePointToml {
    /// Normalized electric output fraction [0, 1].
    pub capacity_ratio: f64,
    /// Efficiency scaling factor at this capacity ratio.
    pub efficiency_ratio: f64,
}

/// Generator part-load efficiency curve loaded from
/// `defaults/generator/efficiency_curve.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GeneratorEfficiencyCurve {
    #[serde(rename = "points")]
    pub points: Vec<GeneratorCurvePointToml>,
}

/// Generic default parameters loaded from a TOML file in any equipment
/// subdirectory.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(transparent)]
pub struct EquipmentDefaults {
    pub params: HashMap<String, toml::Value>,
}

/// Known unit strings for water heater default parameters.
/// Each unit is either a direct SI unit (no conversion) or requires scaling.
const KNOWN_WATER_HEATING_UNITS: &[&str] = &[
    "degC",
    "deltaC",
    "K",
    "W",
    "kW",
    "m3",
    "L",
    "m",
    "W_per_K",
    "m2_K_per_W",
    "L_per_day",
    "kg_per_s",
    "dimensionless",
    // Legacy aliases accepted but equivalent to the canonical form above.
    "C",
];

/// Convert a value from its declared unit to the SI equivalent stored internally.
///
/// Returns the converted value (always in SI). Unrecognized units pass through
/// without conversion — the invariant checker flags them at startup.
///
/// Conversion factors:
/// - kW → W: via [`power_kw_to_w`] (1 kW = 1000 W, NIST SP 330 §7.3)
fn convert_water_heating_value_to_si(value: f64, unit: &str) -> f64 {
    match unit {
        // Power: kW → W conversion. HARES stores electric element power in W
        // (see wh_config.rs validation ranges). If a CSV row declares kW, the
        // value must be multiplied by 1000 to match the SI internal convention.
        "kW" => power_kw_to_w(value),
        // All other known units are already in SI or are dimensionless — no conversion needed.
        "W" | "degC" | "deltaC" | "K" | "C" | "m3" | "L" | "m" | "W_per_K" | "m2_K_per_W"
        | "L_per_day" | "kg_per_s" | "dimensionless" | "" => value,
        unrecognized => {
            tracing::warn!(
                unit = unrecognized,
                value,
                "unrecognized unit in water heating defaults; value passed through \
                 without conversion — verify it is in SI. Known units: {}",
                KNOWN_WATER_HEATING_UNITS.join(", ")
            );
            value
        }
    }
}

/// One row from `defaults/water_heating/default_paramters.csv`.
#[derive(Debug, Clone, PartialEq)]
struct WaterHeatingDefaultRow {
    description: String,
    name: String,
    /// Value in SI units after conversion.
    value: f64,
    /// Unit string as declared in the CSV.
    units: String,
}

/// One row from `defaults/ev/vehicle_mapping.csv`.
///
/// Maps one of the 50 anonymous vehicle columns in `EV Profiles.csv` to its
/// vehicle type, driving-behaviour archetype profile file, and key physical
/// parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleMappingEntry {
    /// Column name in `EV Profiles.csv` (e.g. `Vehicle 1`).
    pub profile_column: String,
    /// Vehicle type (e.g. `MY2030_BEV_SUV`, `MY2030_PHEV_SUV`).
    pub vehicle_type: String,
    /// Per-vehicle driving profile archetype (e.g. `pdf_Veh1`).
    pub profile_file: String,
    /// Battery capacity (kWh).
    pub capacity_kwh: f64,
    /// Maximum onboard charger power (kW).
    pub charger_power_kw: f64,
    /// Grid-to-battery charging efficiency (0.0–1.0).
    pub efficiency: f64,
}

/// Parsed vehicle-to-type mapping loaded from `defaults/ev/vehicle_mapping.csv`.
///
/// The 50 anonymous vehicle columns in `EV Profiles.csv` (`Vehicle 1` through
/// `Vehicle 50`) are all variants of the same two vehicle types
/// (`MY2030_BEV_SUV` at 117.6 kWh and `MY2030_PHEV_SUV` at 14.8 kWh) with
/// stochastic driving-behaviour differences captured by 4 archetype profiles
/// (`pdf_Veh1` through `pdf_Veh4`). There are no distinct physical vehicle
/// models beyond these two types; the 50 columns differ only in their
/// aggregate charging load patterns (frequency, timing, duration).
///
/// Source citations:
/// - Vehicle parameters: capacity, charger power, and efficiency sourced from
///   the BEV/PHEV aggregate session CSV files in `defaults/ev/` (generated via
///   NREL EVI-Pro / EVERMI mid-size SUV 2030 projections)
/// - Vehicle fleet mix: 35 BEV / 15 PHEV (70%/30%), consistent with NREL
///   Electrification Futures Study 2030 medium-electrification scenario
/// - Driving archetypes: round-robin assignment of 4 pdf_Veh profiles across
///   each vehicle type; the non-zero charging-event counts in `EV Profiles.csv`
///   informed the threshold between low-activity vehicles (assigned PHEV) and
///   higher-activity vehicles (assigned BEV)
#[derive(Debug, Clone, Default)]
pub struct VehicleMapping {
    entries: Vec<VehicleMappingEntry>,
}

impl VehicleMapping {
    /// Look up a mapping entry by `EV Profiles.csv` column name.
    #[must_use]
    pub fn get(&self, profile_column: &str) -> Option<&VehicleMappingEntry> {
        self.entries
            .iter()
            .find(|e| e.profile_column == profile_column)
    }

    /// Number of mapping entries loaded.
    #[must_use]
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Iterator over all mapping entries.
    pub fn iter(&self) -> impl Iterator<Item = &VehicleMappingEntry> {
        self.entries.iter()
    }
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

    /// Look up the description text by its CSV row name.
    #[must_use]
    pub fn description(&self, name: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.description.as_str())
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
    zip_by_equipment: HashMap<String, ZipLoad>,
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
    /// EV vehicle-to-type mapping loaded from `defaults/ev/vehicle_mapping.csv`.
    ev_mapping: Option<VehicleMapping>,
    envelope_lut: Option<crate::envelope_lut::EnvelopeLookup>,
    /// Typed generator efficiency curve loaded from
    /// `defaults/generator/efficiency_curve.toml`.
    pub generator_curve: Option<GeneratorEfficiencyCurve>,
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
    #[error("missing critical row '{row_name}' in {path}")]
    MissingRow { path: PathBuf, row_name: String },
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
        store.generator_curve = load_generator_curve(&defaults_dir.join("generator"));
        store.loads = load_toml_dir(&defaults_dir.join("loads"))?;
        store.pv = load_toml_dir(&defaults_dir.join("pv"))?;
        store.pv_panel = load_pv_panel_defaults(&defaults_dir.join("pv"))?;
        store.water_heating = load_toml_dir(&defaults_dir.join("water_heating"))?;
        store.water_heating_csv = load_water_heating_csv(
            &defaults_dir
                .join("water_heating")
                .join("default_paramters.csv"),
        );
        store.ev_mapping = load_vehicle_mapping_csv(&defaults_dir.join("ev"));
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if let Some(ref wh) = store.water_heating_csv {
                check_water_heating_invariants(wh);
            }
            if store.generator_curve.is_none() {
                tracing::warn!(
                    "generator efficiency curve file not loaded; \
                     falling back to hardcoded 6-point default curve"
                );
            }
            check_csv_header_invariants(defaults_dir);
        }

        Ok(store)
    }

    /// Look up ZIP parameters by equipment type name.
    ///
    /// Name matching is canonicalized to lowercase snake case.
    #[must_use]
    pub fn zip_params(&self, equipment_type: &str) -> Option<&ZipLoad> {
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

    /// Access the EV vehicle-to-type mapping loaded from
    /// `defaults/ev/vehicle_mapping.csv`.
    #[must_use]
    pub fn ev_mapping(&self) -> Option<&VehicleMapping> {
        self.ev_mapping.as_ref()
    }

    /// Whether the EV vehicle mapping CSV was loaded successfully.
    #[must_use]
    pub fn has_ev_mapping(&self) -> bool {
        self.ev_mapping.is_some()
    }

    /// Return the loaded generator efficiency curve points, if any,
    /// converted to the runtime [`hares_equipment::GeneratorEfficiencyCurvePoint`] type.
    ///
    /// Returns `None` when the `defaults/generator/efficiency_curve.toml`
    /// file was not loaded (missing, malformed, or validation failed).
    #[must_use]
    pub fn generator_efficiency_curve_points(
        &self,
    ) -> Option<Vec<hares_equipment::GeneratorEfficiencyCurvePoint>> {
        self.generator_curve.as_ref().map(|curve| {
            curve
                .points
                .iter()
                .map(|p| hares_equipment::GeneratorEfficiencyCurvePoint {
                    capacity_ratio: p.capacity_ratio,
                    efficiency_ratio: p.efficiency_ratio,
                })
                .collect()
        })
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

fn load_zip_parameters(path: &Path) -> Result<HashMap<String, ZipLoad>, DefaultsError> {
    let content = std::fs::read_to_string(path).map_err(|e| DefaultsError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let table: HashMap<String, ZipLoad> =
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
            Some("csv") => {
                let curve_set = load_hvac_csv_file(&path)?;
                map.insert(stem, curve_set);
            }
            _ => {}
        }
    }
    Ok(map)
}

/// Row names that must be present in every HVAC biquadratic CSV file for the
/// curve set to be physically valid. These are the biquadratic and quadratic
/// coefficient rows that define the equipment's capacity and EIR performance
/// curves. If any of these rows is absent — e.g. due to a typo like
/// `a_eirr_t` instead of `a_eir_t` — the loader returns
/// [`DefaultsError::MissingRow`] rather than silently substituting zeros,
/// because an all-zero coefficient set produces physically impossible results
/// (e.g. EIR = 0 → zero electricity consumption).
///
/// Non-critical rows (temperature bounds, flow-fraction bounds, PLF bounds)
/// have documented fallback values and are not in this list; their absence
/// emits a `tracing::warn!` but does not fail the load.
const CRITICAL_HVAC_ROWS: &[&str] = &[
    // EIR-temperature biquadratic coefficients [a, b, c, d, e, f]
    "a_eir_t",
    "b_eir_t",
    "c_eir_t",
    "d_eir_t",
    "e_eir_t",
    "f_eir_t",
    // EIR-flow-fraction quadratic coefficients [a, b, c]
    "a_eir_ff",
    "b_eir_ff",
    "c_eir_ff",
    // EIR-part-load-ratio quadratic coefficients [a, b, c]
    "a_eir_plr",
    "b_eir_plr",
    "c_eir_plr",
    // Capacity-temperature biquadratic coefficients [a, b, c, d, e, f]
    "a_cap_t",
    "b_cap_t",
    "c_cap_t",
    "d_cap_t",
    "e_cap_t",
    "f_cap_t",
    // Capacity-flow-fraction quadratic coefficients [a, b, c]
    "a_cap_ff",
    "b_cap_ff",
    "c_cap_ff",
];

/// Non-critical row names that have documented fallback values. Their absence
/// emits a `tracing::warn!` but does not fail the load.
const NON_CRITICAL_HVAC_ROWS: &[&str] = &[
    "min_Twb", "max_Twb", "min_Tdb", "max_Tdb", "min_ff", "max_ff", "min_plf", "max_plf",
];

/// OCHRE uses ±100 (°F) as a sentinel for "effectively unbounded" temperature
/// range in its HVAC heating CSV defaults
/// (`vendors/OCHRE/ochre/defaults/HVAC Heating/Biquadratic *.csv:23-26`).
/// The value is in Fahrenheit — OCHRE's internal unit — but HARES operates in
/// Celsius (SI) internally. A ±100 °C bound has no physical meaning for any
/// heat pump and would let biquadratic curves extrapolate far outside their
/// valid domain, producing implausible capacity/EIR predictions. We detect the
/// sentinel at the I/O boundary and replace it with physically meaningful
/// Celsius fallback bounds so no Fahrenheit value ever reaches the core.
const FAHRENHEIT_SENTINEL_MIN: f64 = -100.0;
const FAHRENHEIT_SENTINEL_MAX: f64 = 100.0;

/// Fallback Celsius temperature bounds used when a Fahrenheit sentinel is
/// detected or when temperature-bound rows are absent from a CSV. These match
/// the `get_row_with_default` defaults below and represent a wide but
/// physically meaningful operating range for residential HVAC equipment.
const FALLBACK_TWB_BOUNDS: (f64, f64) = (-10.0, 50.0);
const FALLBACK_TDB_BOUNDS: (f64, f64) = (-50.0, 60.0);

/// Check whether a `(min, max)` bound pair matches the OCHRE ±100 °F sentinel
/// convention for "unbounded" temperature range.
fn is_fahrenheit_sentinel(min: f64, max: f64) -> bool {
    min == FAHRENHEIT_SENTINEL_MIN && max == FAHRENHEIT_SENTINEL_MAX
}

/// Sanitise a single `(min, max)` temperature bound pair: if it matches the
/// OCHRE ±100 °F sentinel, replace it with the given Celsius fallback and
/// return `true` (sentinel detected). Otherwise return the original bounds
/// unchanged and `false`.
///
/// `bound_label` (e.g. `"Twb"`, `"Tdb"`) and `file_path` are used in the
/// warning so the operator can identify which file and which bound triggered
/// the replacement.
fn sanitise_sentinel_bounds(
    min: f64,
    max: f64,
    fallback: (f64, f64),
    bound_label: &str,
    file_path: &Path,
) -> (f64, f64, bool) {
    if is_fahrenheit_sentinel(min, max) {
        tracing::warn!(
            path = %file_path.display(),
            bound = %bound_label,
            raw_min = min,
            raw_max = max,
            fallback_min = fallback.0,
            fallback_max = fallback.1,
            "Fahrenheit sentinel (±100) detected in temperature bounds; \
             replacing with Celsius fallback",
        );
        #[cfg(feature = "observe")]
        tracing::info!(
            target: "observe",
            column = "hvac_sentinel_replaced",
            path = %file_path.display(),
            bound = %bound_label,
            count = 1u32,
            "Fahrenheit sentinel replaced with Celsius fallback",
        );
        (fallback.0, fallback.1, true)
    } else {
        (min, max, false)
    }
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
        .map(|v| {
            let (twb_min, twb_max, _) = sanitise_sentinel_bounds(
                v.twb_bounds[0],
                v.twb_bounds[1],
                FALLBACK_TWB_BOUNDS,
                "Twb",
                path,
            );
            let (tdb_min, tdb_max, _) = sanitise_sentinel_bounds(
                v.tdb_bounds[0],
                v.tdb_bounds[1],
                FALLBACK_TDB_BOUNDS,
                "Tdb",
                path,
            );
            HvacCurveVariant {
                name: v.name,
                cap_t: BiquadraticCurve {
                    coeffs: v.cap_t,
                    x1_bounds: (twb_min, twb_max),
                    x2_bounds: (tdb_min, tdb_max),
                    warn_on_clamp: true,
                    output_min: Some(0.0),
                    output_max: None,
                },
                cap_ff: v.cap_ff,
                eir_t: BiquadraticCurve {
                    coeffs: v.eir_t,
                    x1_bounds: (twb_min, twb_max),
                    x2_bounds: (tdb_min, tdb_max),
                    warn_on_clamp: true,
                    output_min: None,
                    output_max: None,
                },
                eir_ff: v.eir_ff,
                eir_plr: v.eir_plr,
                ff_bounds: None,
                plf_bounds: None,
            }
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
    // Each cell that is missing or fails to parse emits a `tracing::warn!`
    // with the row name, column index, and raw value, then falls back to 0.0.
    // The total count of zero-fallback events is recorded for observability.
    let mut data: HashMap<String, Vec<f64>> = HashMap::new();
    let mut zero_fallback_count = 0u32;
    for result in rdr.records() {
        let record = result.map_err(|e| DefaultsError::MalformedToml {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
        let row_name = record.get(0).unwrap_or("").to_string();
        let mut values = Vec::with_capacity(n_variants);
        for i in 1..=n_variants {
            match record.get(i) {
                None => {
                    tracing::warn!(
                        path = %path.display(),
                        row = %row_name,
                        column = i,
                        "missing column in HVAC CSV row; using 0.0 fallback",
                    );
                    zero_fallback_count += 1;
                    values.push(0.0);
                }
                Some(raw) => match raw.parse::<f64>() {
                    Ok(v) => values.push(v),
                    Err(_) => {
                        tracing::warn!(
                            path = %path.display(),
                            row = %row_name,
                            column = i,
                            raw_value = raw,
                            "unparseable value in HVAC CSV row; using 0.0 fallback",
                        );
                        zero_fallback_count += 1;
                        values.push(0.0);
                    }
                },
            }
        }
        data.insert(row_name, values);
    }

    // Fail the entire file load if any critical coefficient row is absent.
    // A missing critical row (e.g. `a_eir_t` misspelled as `a_eirr_t`) would
    // silently produce all-zero coefficients, yielding physically impossible
    // results like EIR = 0 (zero electricity consumption).
    for &name in CRITICAL_HVAC_ROWS {
        if !data.contains_key(name) {
            return Err(DefaultsError::MissingRow {
                path: path.to_path_buf(),
                row_name: name.to_string(),
            });
        }
    }

    // Warn on missing non-critical rows that have documented fallback values.
    for &name in NON_CRITICAL_HVAC_ROWS {
        if !data.contains_key(name) {
            tracing::warn!(
                path = %path.display(),
                row = %name,
                "non-critical row missing in HVAC CSV; using fallback values",
            );
        }
    }

    // Observer capture: record the total zero-fallback count for this file
    // so monitoring can alert on data-quality regressions in shipped CSVs.
    #[cfg(feature = "observe")]
    if zero_fallback_count > 0 {
        tracing::info!(
            target: "observe",
            column = "hvac_csv_zero_fallback",
            path = %path.display(),
            count = zero_fallback_count,
            "zero-fallback events during HVAC CSV load",
        );
    }
    // Why: zero_fallback_count is only read by the observe feature path;
    // without that feature the count is accumulated but never consumed.
    // The count tracking is always compiled to keep the parsing logic
    // cfg-free, so we explicitly discard the value when observe is off.
    #[cfg(not(feature = "observe"))]
    let _ = zero_fallback_count;

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
            let twb_min = get_row_with_default("min_Twb", FALLBACK_TWB_BOUNDS.0)[i];
            let twb_max = get_row_with_default("max_Twb", FALLBACK_TWB_BOUNDS.1)[i];
            let tdb_min = get_row_with_default("min_Tdb", FALLBACK_TDB_BOUNDS.0)[i];
            let tdb_max = get_row_with_default("max_Tdb", FALLBACK_TDB_BOUNDS.1)[i];

            // Sanitise OCHRE ±100 °F sentinel values at the I/O boundary so
            // no Fahrenheit value reaches the internal (Celsius) model.
            let (twb_min, twb_max, _) =
                sanitise_sentinel_bounds(twb_min, twb_max, FALLBACK_TWB_BOUNDS, "Twb", path);
            let (tdb_min, tdb_max, _) =
                sanitise_sentinel_bounds(tdb_min, tdb_max, FALLBACK_TDB_BOUNDS, "Tdb", path);

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
    for (row_idx, rec) in rdr.deserialize::<HashMap<String, String>>().enumerate() {
        let record = rec.map_err(|e| DefaultsError::MalformedToml {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        let hvac_name = record
            .get("HVAC Name")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if hvac_name.is_empty() {
            tracing::warn!(
                path = %path.display(),
                record = row_idx + 1,
                "skipping multispeed CSV row with missing or empty 'HVAC Name' column",
            );
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
/// Values declared in non-SI units (e.g. kW) are converted to SI at load time.
/// Returns `None` if the file does not exist (non-fatal — callers fall back).
fn load_water_heating_csv(path: &Path) -> Option<WaterHeatingDefaults> {
    if !path.exists() {
        return None;
    }
    let mut rdr = match csv::ReaderBuilder::new()
        .comment(Some(b'#'))
        .from_path(path)
    {
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
                let si_value = convert_water_heating_value_to_si(v, &units);
                #[cfg(feature = "observe")]
                {
                    tracing::info!(
                        target: "hares_io::defaults::water_heating",
                        name = %name,
                        raw_value = v,
                        units = %units,
                        si_value,
                        "water heating default parameter loaded"
                    );
                }
                rows.push(WaterHeatingDefaultRow {
                    description,
                    name,
                    value: si_value,
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

/// Load EV vehicle-to-type mapping from `defaults/ev/vehicle_mapping.csv`.
///
/// The CSV must have columns: `profile_column`, `vehicle_type`, `profile_file`,
/// `capacity_kwh`, `charger_power_kw`, `efficiency`.
///
/// Returns `None` if the file does not exist (non-fatal — the mapping is a
/// data-integrity guard, not a required startup resource). However, if the file
/// exists but has fewer than 50 rows (one per Vehicle 1–50), a `tracing::error!`
/// is emitted and `None` is returned.
///
/// Malformed rows (unparseable numeric fields) are skipped with a warning.
fn load_vehicle_mapping_csv(ev_dir: &Path) -> Option<VehicleMapping> {
    let path = ev_dir.join("vehicle_mapping.csv");
    if !path.exists() {
        return None;
    }
    let mut rdr = match csv::ReaderBuilder::new()
        .comment(Some(b'#'))
        .from_path(&path)
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to open EV vehicle mapping CSV"
            );
            return None;
        }
    };
    let mut entries: Vec<VehicleMappingEntry> = Vec::new();
    for result in rdr.records() {
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping malformed row in EV vehicle mapping CSV"
                );
                continue;
            }
        };
        let profile_column = record.get(0).unwrap_or("").trim().to_string();
        let vehicle_type = record.get(1).unwrap_or("").trim().to_string();
        let profile_file = record.get(2).unwrap_or("").trim().to_string();
        let capacity_str = record.get(3).unwrap_or("");
        let charger_str = record.get(4).unwrap_or("");
        let efficiency_str = record.get(5).unwrap_or("");

        if profile_column.is_empty() || vehicle_type.is_empty() || profile_file.is_empty() {
            tracing::warn!(
                profile_column = %profile_column,
                "skipping EV vehicle mapping row with empty required fields"
            );
            continue;
        }

        let capacity_kwh = match capacity_str.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            Ok(_) => {
                tracing::warn!(
                    profile_column = %profile_column,
                    capacity_kwh = %capacity_str.trim(),
                    "non-positive capacity_kwh in EV vehicle mapping CSV"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    profile_column = %profile_column,
                    capacity_kwh = %capacity_str.trim(),
                    error = %e,
                    "unparseable capacity_kwh in EV vehicle mapping CSV"
                );
                continue;
            }
        };

        let charger_power_kw = match charger_str.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            Ok(_) => {
                tracing::warn!(
                    profile_column = %profile_column,
                    charger_power_kw = %charger_str.trim(),
                    "non-positive charger_power_kw in EV vehicle mapping CSV"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    profile_column = %profile_column,
                    charger_power_kw = %charger_str.trim(),
                    error = %e,
                    "unparseable charger_power_kw in EV vehicle mapping CSV"
                );
                continue;
            }
        };

        let efficiency = match efficiency_str.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 && v <= 1.0 => v,
            Ok(v) if v.is_finite() => {
                tracing::warn!(
                    profile_column = %profile_column,
                    efficiency = v,
                    "EV vehicle mapping efficiency {v} not in range (0.0, 1.0] — check data"
                );
                continue;
            }
            Ok(_) => {
                tracing::warn!(
                    profile_column = %profile_column,
                    efficiency = %efficiency_str.trim(),
                    "non-finite efficiency value in EV vehicle mapping CSV"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(
                    profile_column = %profile_column,
                    efficiency = %efficiency_str.trim(),
                    error = %e,
                    "unparseable efficiency in EV vehicle mapping CSV"
                );
                continue;
            }
        };

        entries.push(VehicleMappingEntry {
            profile_column,
            vehicle_type,
            profile_file,
            capacity_kwh,
            charger_power_kw,
            efficiency,
        });
    }

    if entries.len() < 50 {
        // EV Profiles.csv has exactly 50 vehicle columns (Vehicle 1–50).
        // Fewer than 50 mapping entries means some columns have no type
        // association and would silently produce None at query time.
        let expected = 50;
        let actual = entries.len();
        tracing::error!(
            expected,
            actual,
            "EV vehicle mapping CSV has only {actual} of {expected} expected \
             entries (one per Vehicle 1–50 in EV Profiles.csv). Simulation \
             results will be incorrect if unmapped vehicle columns are used — \
             apply the correct vehicle type to every column in \
             defaults/ev/vehicle_mapping.csv.",
        );
        return None;
    }

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        check_ev_mapping_invariants(&entries);
    }

    Some(VehicleMapping { entries })
}

/// Invariant checks for EV vehicle mapping loaded at startup.
///
/// Validates:
/// - Every expected vehicle column (Vehicle 1 through Vehicle 50) has exactly
///   one mapping entry.
/// - No duplicate `profile_column` values.
/// - Every `vehicle_type` is one of the two known types.
/// - Every `profile_file` is one of the four known archetypes.
/// - `capacity_kwh` values match the vehicle type's known capacity.
/// - `efficiency` is in (0.0, 1.0].
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_ev_mapping_invariants(entries: &[VehicleMappingEntry]) {
    use std::collections::HashSet;

    let expected_columns: HashSet<String> = (1..=50).map(|i| format!("Vehicle {i}")).collect();
    let actual_columns: HashSet<&str> = entries.iter().map(|e| e.profile_column.as_str()).collect();

    // Check for missing vehicle columns.
    let missing: Vec<String> = expected_columns
        .iter()
        .filter(|c| !actual_columns.contains(c.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        tracing::error!(
            missing = ?missing,
            "EV vehicle mapping CSV is missing entries for {} vehicle \
             column(s). Every Vehicle 1–50 in EV Profiles.csv must have \
             a corresponding entry in vehicle_mapping.csv.",
            missing.len()
        );
    }

    // Check for duplicate profile_column values.
    let mut seen: HashSet<&str> = HashSet::new();
    for entry in entries {
        if !seen.insert(&entry.profile_column) {
            tracing::error!(
                profile_column = %entry.profile_column,
                "duplicate profile_column in EV vehicle mapping CSV"
            );
        }
    }

    // Check vehicle_type values.
    const KNOWN_TYPES: &[&str] = &["MY2030_BEV_SUV", "MY2030_PHEV_SUV"];
    for entry in entries {
        if !KNOWN_TYPES.contains(&entry.vehicle_type.as_str()) {
            tracing::warn!(
                profile_column = %entry.profile_column,
                vehicle_type = %entry.vehicle_type,
                known_types = ?KNOWN_TYPES,
                "unrecognized vehicle_type in EV vehicle mapping CSV"
            );
        }
    }

    // Check profile_file values.
    const KNOWN_PROFILES: &[&str] = &["pdf_Veh1", "pdf_Veh2", "pdf_Veh3", "pdf_Veh4"];
    for entry in entries {
        if !KNOWN_PROFILES.contains(&entry.profile_file.as_str()) {
            tracing::warn!(
                profile_column = %entry.profile_column,
                profile_file = %entry.profile_file,
                known_profiles = ?KNOWN_PROFILES,
                "unrecognized profile_file in EV vehicle mapping CSV"
            );
        }
    }

    // Check capacity_kwh matches vehicle_type.
    // BEV_SUV: 117.6 kWh (NREL EVI-Pro/EVERMI mid-size SUV 2030 projection)
    // PHEV_SUV: 14.8 kWh (NREL EVI-Pro/EVERMI mid-size SUV 2030 projection)
    for entry in entries {
        let expected_capacity = match entry.vehicle_type.as_str() {
            "MY2030_BEV_SUV" => 117.6,
            "MY2030_PHEV_SUV" => 14.8,
            _ => continue, // unrecognized types flagged above
        };
        if (entry.capacity_kwh - expected_capacity).abs() > 0.01 {
            tracing::warn!(
                profile_column = %entry.profile_column,
                vehicle_type = %entry.vehicle_type,
                capacity_kwh = entry.capacity_kwh,
                expected_capacity_kwh = expected_capacity,
                "capacity_kwh deviates from expected value for vehicle_type"
            );
        }
    }

    // Check efficiency range.
    for entry in entries {
        if entry.efficiency <= 0.0 || entry.efficiency > 1.0 {
            tracing::error!(
                profile_column = %entry.profile_column,
                efficiency = entry.efficiency,
                "efficiency must be in (0.0, 1.0]"
            );
        }
    }
}

/// Invariant checks for water heater defaults loaded at startup.
///
/// Validates:
/// - UA values are within the physically plausible range (0.5–5.0 W/K) for residential tanks
/// - UA values increase monotonically with tank volume (larger tanks have higher heat loss)
/// - UEF values are within valid ranges (0.0–1.0 for electric/gas, 1.0–5.0 for HPWH)
/// - Tank volumes are in standard sizes with correct gallon-to-liter conversions
/// - Every required field has a corresponding entry
/// - Every row has a non-empty Units column
/// - Every unit string is in the known set; unrecognized units produce a warning
///
/// Core invariants that would corrupt simulation results produce `tracing::error!`;
/// minor anomalies produce `tracing::warn!`.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_water_heating_invariants(wh: &WaterHeatingDefaults) {
    // Unit validation: every row must have a non-empty, recognized unit.
    for row in &wh.rows {
        if row.units.is_empty() {
            tracing::error!(
                row_name = %row.name,
                row_value = row.value,
                "water heating defaults row has empty Units column; \
                 use 'dimensionless' for dimensionless parameters"
            );
        } else if !KNOWN_WATER_HEATING_UNITS.contains(&row.units.as_str()) {
            tracing::warn!(
                row_name = %row.name,
                row_unit = %row.units,
                "unrecognized unit for water heating defaults row; \
                 known units: {}",
                KNOWN_WATER_HEATING_UNITS.join(", ")
            );
        }
    }

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

/// Check equipment defaults directories for CSV files whose parameter names
/// belong to a different equipment type — for example, a battery configuration
/// file placed in the generator directory.
///
/// Unrecognised CSV files are not consumed by the loader (which only reads
/// `.toml` from equipment directories via [`load_toml_dir`]), but they create
/// confusion and can mislead users. This check emits a warning for each
/// misplacement detected so the operator can clean it up.
///
/// The check inspects CSV files in the generator directory and flags any file
/// whose "Name" column contains battery-specific parameter names.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
fn check_csv_header_invariants(defaults_dir: &Path) {
    // Battery-specific parameters that should never appear in non-battery
    // equipment parameter files. Derived from defaults/battery/default_parameters.csv
    // and battery/ directory files.
    const BATTERY_ONLY_PARAMS: &[&str] = &[
        "capacity_kwh",
        "soc_init",
        "soc_max",
        "soc_min",
        "efficiency_charge",
        "efficiency_discharge",
        "efficiency_inverter",
        "discharge_pct",
        "initial_voltage",
        "v_cell",
        "ah_cell",
        "r_cell",
        "charge_start_hour",
        "discharge_start_hour",
        "charge_power",
        "discharge_power",
        "charge_from_solar",
        "import_limit",
        "export_limit",
        "thermal_r",
        "thermal_c",
    ];

    let findings =
        check_dir_for_foreign_csv_params(&defaults_dir.join("generator"), BATTERY_ONLY_PARAMS);
    for finding in &findings {
        tracing::warn!(
            directory = %defaults_dir.join("generator").display(),
            finding = %finding,
            "misplaced battery parameter file detected in generator defaults directory"
        );
    }
}

/// Scan an equipment defaults directory for CSV files whose "Name" column
/// values include parameter names that belong to a different equipment type.
///
/// Returns human-readable strings describing each violation found.
/// Callers convert these to warnings, errors, or test assertions.
#[cfg(any(test, debug_assertions, feature = "check_invariants"))]
fn check_dir_for_foreign_csv_params(dir: &Path, foreign_params: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    if !dir.exists() {
        return findings;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return findings;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"))
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut reader = csv::Reader::from_reader(content.as_bytes());
        let headers = match reader.headers() {
            Ok(h) => h.clone(),
            Err(_) => continue,
        };

        // Find the index of the "Name" column (case-insensitive).
        // OCHRE-format CSV files use the "Name" column for parameter names.
        // For wide-format CSVs where headers are the parameter names, check
        // the headers directly.
        let name_idx = headers.iter().position(|h| h.eq_ignore_ascii_case("Name"));

        let mut foreign_names: Vec<String> = Vec::new();

        if let Some(name_idx) = name_idx {
            for result in reader.records() {
                let Ok(record) = result else {
                    continue;
                };
                if let Some(name) = record.get(name_idx) {
                    let name_lower = name.trim().to_ascii_lowercase();
                    if foreign_params.contains(&name_lower.as_str()) {
                        let clean = name.trim().to_string();
                        if !foreign_names.contains(&clean) {
                            foreign_names.push(clean);
                        }
                    }
                }
            }
        } else {
            // No "Name" column — check header columns directly against
            // foreign parameter names.
            for h in headers.iter() {
                let h_lower = h.trim().to_ascii_lowercase();
                if foreign_params.contains(&h_lower.as_str()) {
                    let clean = h.trim().to_string();
                    if !foreign_names.contains(&clean) {
                        foreign_names.push(clean);
                    }
                }
            }
        }

        if !foreign_names.is_empty() {
            findings.push(format!(
                "file '{}' contains battery-specific parameters {foreign_names:?} — \
                 this file does not belong in {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                dir.display()
            ));
        }
    }
    findings
}

/// Load the generator efficiency curve from
/// `defaults/generator/efficiency_curve.toml`.
///
/// Returns `None` silently when the file is missing — the caller falls back to
/// the hardcoded 6-point curve at
/// [`hares_equipment::generator::EfficiencyModel::default_curve_points`].
fn load_generator_curve(dir: &Path) -> Option<GeneratorEfficiencyCurve> {
    let path = dir.join("efficiency_curve.toml");
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<GeneratorEfficiencyCurve>(&content) {
            Ok(curve) => {
                // Validate the points at load time.
                if curve.points.len() < 2 {
                    tracing::warn!(
                        path = %path.display(),
                        "generator efficiency curve has fewer than 2 points; ignoring"
                    );
                    return None;
                }
                for point in &curve.points {
                    if !point.capacity_ratio.is_finite()
                        || !(0.0..=1.0).contains(&point.capacity_ratio)
                        || !point.efficiency_ratio.is_finite()
                        || point.efficiency_ratio < 0.0
                    {
                        tracing::warn!(
                            path = %path.display(),
                            capacity_ratio = point.capacity_ratio,
                            efficiency_ratio = point.efficiency_ratio,
                            "invalid generator efficiency curve point; ignoring file"
                        );
                        return None;
                    }
                }
                for window in curve.points.windows(2) {
                    if window[1].capacity_ratio <= window[0].capacity_ratio {
                        tracing::warn!(
                            path = %path.display(),
                            "generator efficiency curve points not strictly increasing; ignoring"
                        );
                        return None;
                    }
                }
                Some(curve)
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "malformed generator efficiency curve TOML; ignoring"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "I/O error reading generator efficiency curve; ignoring"
            );
            None
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

    /// Drift test: `defaults/zip_parameters.toml` and the in-code class table
    /// `hares_types::zip::zip_defaults_for_class` are dual representations of
    /// the same data. Every class name must resolve to the same coefficients
    /// through both, and the toml must contain no orphan rows.
    #[test]
    fn zip_parameters_toml_matches_class_table_exactly() {
        use hares_types::zip::{ZIP_CLASS_NAMES, zip_defaults_for_class};

        let toml_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults")
            .join("zip_parameters.toml");
        let table = load_zip_parameters(&toml_path).expect("load zip_parameters.toml");

        let mut covered_keys = std::collections::HashSet::new();
        for &name in ZIP_CLASS_NAMES {
            let expected = zip_defaults_for_class(name)
                .unwrap_or_else(|| panic!("class table missing row for {name:?}"));
            let key = normalize_equipment_key(name);
            let actual = table.get(&key).unwrap_or_else(|| {
                panic!("zip_parameters.toml missing row [{key}] for class {name:?}")
            });
            assert_eq!(
                *actual, expected,
                "toml row [{key}] diverged from zip_defaults_for_class({name:?})"
            );
            covered_keys.insert(key);
        }

        for key in table.keys() {
            assert!(
                covered_keys.contains(key),
                "zip_parameters.toml row [{key}] has no matching class in \
                 zip_defaults_for_class — add the class-table row or delete the toml row"
            );
        }
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
        assert_eq!(v.cap_t.x1_bounds, FALLBACK_TWB_BOUNDS);
        assert_eq!(v.cap_t.x2_bounds, FALLBACK_TDB_BOUNDS);
        assert_eq!(v.plf_bounds, Some((0.48, 1.0)));
        assert_eq!(v.ff_bounds, None);
    }

    /// Write a minimal single-variant HVAC CSV with the given temperature
    /// bound rows. All coefficient rows are identity curves (output 1.0).
    fn write_minimal_hvac_csv(path: &Path, twb: (f64, f64), tdb: (f64, f64)) {
        let (twb_min, twb_max) = twb;
        let (tdb_min, tdb_max) = tdb;
        std::fs::write(
            path,
            format!(
                "Name,Single_1\n\
a_eir_t,1.0\n\
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
min_Twb,{twb_min}\n\
max_Twb,{twb_max}\n\
min_Tdb,{tdb_min}\n\
max_Tdb,{tdb_max}\n",
            ),
        )
        .unwrap();
    }

    #[test]
    fn heating_bounds_sentinel_detected_and_replaced_with_celsius_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("ashp_heater.csv");
        write_minimal_hvac_csv(&csv_path, (-100.0, 100.0), (-100.0, 100.0));

        let set = load_hvac_csv_file(&csv_path).expect("csv should parse");
        let v = &set.variants[0];

        assert_eq!(
            v.cap_t.x1_bounds, FALLBACK_TWB_BOUNDS,
            "sentinel Twb bounds should be replaced with Celsius fallback"
        );
        assert_eq!(
            v.cap_t.x2_bounds, FALLBACK_TDB_BOUNDS,
            "sentinel Tdb bounds should be replaced with Celsius fallback"
        );
        assert_eq!(v.eir_t.x1_bounds, FALLBACK_TWB_BOUNDS);
        assert_eq!(v.eir_t.x2_bounds, FALLBACK_TDB_BOUNDS);
    }

    #[test]
    fn cooling_bounds_not_flagged_as_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("ashp_cooler.csv");
        write_minimal_hvac_csv(&csv_path, (13.88, 23.88), (18.33, 51.66));

        let set = load_hvac_csv_file(&csv_path).expect("csv should parse");
        let v = &set.variants[0];

        assert_eq!(
            v.cap_t.x1_bounds,
            (13.88, 23.88),
            "physically correct Celsius Twb bounds must be preserved"
        );
        assert_eq!(
            v.cap_t.x2_bounds,
            (18.33, 51.66),
            "physically correct Celsius Tdb bounds must be preserved"
        );
        assert_eq!(v.eir_t.x1_bounds, (13.88, 23.88));
        assert_eq!(v.eir_t.x2_bounds, (18.33, 51.66));
    }

    #[test]
    fn non_sentinel_negative_bounds_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("cold_climate_heater.csv");
        write_minimal_hvac_csv(&csv_path, (-20.0, 30.0), (-20.0, 50.0));

        let set = load_hvac_csv_file(&csv_path).expect("csv should parse");
        let v = &set.variants[0];

        assert_eq!(
            v.cap_t.x1_bounds,
            (-20.0, 30.0),
            "genuinely intended negative Celsius bounds must not be replaced"
        );
        assert_eq!(
            v.cap_t.x2_bounds,
            (-20.0, 50.0),
            "genuinely intended negative Celsius bounds must not be replaced"
        );
        assert_eq!(v.eir_t.x1_bounds, (-20.0, 30.0));
        assert_eq!(v.eir_t.x2_bounds, (-20.0, 50.0));
    }

    // ── Silent zero-fallback regression tests (T-0395) ──────────────────

    /// Write a complete single-variant HVAC CSV with all critical rows and
    /// explicit temperature bounds. Coefficient values are distinct non-zero
    /// values so that a zero-fallback is immediately detectable.
    fn write_complete_hvac_csv(path: &Path) {
        std::fs::write(
            path,
            "Name,Single_1\n\
             a_eir_t,-0.30428\n\
             b_eir_t,0.11805\n\
             c_eir_t,-0.00342\n\
             d_eir_t,-0.00626\n\
             e_eir_t,0.0007\n\
             f_eir_t,-0.00047\n\
             a_eir_ff,1.32299905\n\
             b_eir_ff,-0.477711207\n\
             c_eir_ff,0.154712157\n\
             a_eir_plr,0.93\n\
             b_eir_plr,0.07\n\
             c_eir_plr,0.0\n\
             a_cap_t,1.5509\n\
             b_cap_t,-0.07505\n\
             c_cap_t,0.0031\n\
             d_cap_t,0.0024\n\
             e_cap_t,-0.00005\n\
             f_cap_t,-0.00043\n\
             a_cap_ff,0.718605468\n\
             b_cap_ff,0.41009989\n\
             c_cap_ff,-0.128705457\n\
             min_Twb,13.88\n\
             max_Twb,23.88\n\
             min_Tdb,18.33\n\
             max_Tdb,51.66\n",
        )
        .unwrap();
    }

    #[test]
    fn missing_critical_row_fails_load() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("bad.csv");
        write_complete_hvac_csv(&csv_path);

        // Remove the `a_eir_t` line to simulate a typo (e.g. `a_eirr_t`).
        let content = std::fs::read_to_string(&csv_path).unwrap();
        let modified = content
            .lines()
            .filter(|line| !line.starts_with("a_eir_t,"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&csv_path, modified).unwrap();

        let result = load_hvac_csv_file(&csv_path);
        assert!(
            matches!(
                &result,
                Err(DefaultsError::MissingRow { row_name, .. }) if row_name == "a_eir_t"
            ),
            "missing a_eir_t row must return MissingRow error, got: {result:?}",
        );
    }

    #[test]
    fn critical_row_missing_propagates_through_load_hvac_curves_dir() {
        let dir = tempfile::tempdir().unwrap();
        let hvac_dir = dir.path().join("hvac_cooling");
        std::fs::create_dir(&hvac_dir).unwrap();
        let csv_path = hvac_dir.join("bad.csv");
        write_complete_hvac_csv(&csv_path);

        let content = std::fs::read_to_string(&csv_path).unwrap();
        let modified = content
            .lines()
            .filter(|line| !line.starts_with("a_eir_t,"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&csv_path, modified).unwrap();

        let result = load_hvac_curves_dir(&hvac_dir);
        assert!(
            matches!(
                &result,
                Err(DefaultsError::MissingRow { row_name, .. }) if row_name == "a_eir_t"
            ),
            "missing a_eir_t row must propagate as MissingRow error through load_hvac_curves_dir, got: {result:?}",
        );
    }

    #[test]
    fn missing_noncritical_row_loads_with_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("no_bounds.csv");
        write_complete_hvac_csv(&csv_path);

        // Strip the four temperature-bound rows (non-critical).
        let content = std::fs::read_to_string(&csv_path).unwrap();
        let modified = content
            .lines()
            .filter(|line| {
                !line.starts_with("min_Twb,")
                    && !line.starts_with("max_Twb,")
                    && !line.starts_with("min_Tdb,")
                    && !line.starts_with("max_Tdb,")
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&csv_path, modified).unwrap();

        let set = load_hvac_csv_file(&csv_path).expect("non-critical rows missing must still load");
        let v = &set.variants[0];

        // Fallback bounds must be applied when temperature rows are absent.
        assert_eq!(v.cap_t.x1_bounds, FALLBACK_TWB_BOUNDS);
        assert_eq!(v.cap_t.x2_bounds, FALLBACK_TDB_BOUNDS);
        assert_eq!(v.eir_t.x1_bounds, FALLBACK_TWB_BOUNDS);
        assert_eq!(v.eir_t.x2_bounds, FALLBACK_TDB_BOUNDS);
    }

    #[test]
    fn unparseable_value_produces_zero_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("bad_value.csv");
        write_complete_hvac_csv(&csv_path);

        // Replace the `a_eir_t` value with an unparseable string.
        let content = std::fs::read_to_string(&csv_path).unwrap();
        let modified = content.replace("a_eir_t,-0.30428", "a_eir_t,abc");
        std::fs::write(&csv_path, modified).unwrap();

        let set = load_hvac_csv_file(&csv_path).expect("unparseable value must not fail the load");
        let v = &set.variants[0];

        // The unparseable cell must fall back to 0.0 while the rest of the
        // row's coefficients are parsed correctly.
        assert!(
            (v.eir_t.coeffs[0] - 0.0).abs() < f64::EPSILON,
            "unparseable a_eir_t must be 0.0, got {}",
            v.eir_t.coeffs[0],
        );
        assert!(
            (v.eir_t.coeffs[1] - 0.11805).abs() < 1e-10,
            "b_eir_t must still be parsed correctly, got {}",
            v.eir_t.coeffs[1],
        );
    }

    #[test]
    fn well_formed_csv_produces_correct_coefficients() {
        let dir = tempfile::tempdir().unwrap();
        let csv_path = dir.path().join("good.csv");
        write_complete_hvac_csv(&csv_path);

        let set = load_hvac_csv_file(&csv_path).expect("well-formed CSV must load");
        let v = &set.variants[0];

        // Every coefficient must match the CSV exactly — if any zero-fallback
        // had occurred, the coefficient would be 0.0 instead.
        assert!((v.eir_t.coeffs[0] - (-0.30428)).abs() < 1e-10);
        assert!((v.eir_t.coeffs[1] - 0.11805).abs() < 1e-10);
        assert!((v.eir_t.coeffs[2] - (-0.00342)).abs() < 1e-10);
        assert!((v.eir_t.coeffs[3] - (-0.00626)).abs() < 1e-10);
        assert!((v.eir_t.coeffs[4] - 0.0007).abs() < 1e-10);
        assert!((v.eir_t.coeffs[5] - (-0.00047)).abs() < 1e-10);

        assert!((v.cap_t.coeffs[0] - 1.5509).abs() < 1e-10);
        assert!((v.cap_t.coeffs[1] - (-0.07505)).abs() < 1e-10);
        assert!((v.cap_t.coeffs[2] - 0.0031).abs() < 1e-10);

        assert!((v.cap_ff[0] - 0.718605468).abs() < 1e-10);
        assert!((v.eir_ff[0] - 1.32299905).abs() < 1e-10);
        assert!((v.eir_plr[0] - 0.93).abs() < 1e-10);

        // Temperature bounds must be preserved exactly.
        assert_eq!(v.cap_t.x1_bounds, (13.88, 23.88));
        assert_eq!(v.cap_t.x2_bounds, (18.33, 51.66));
    }

    #[test]
    fn all_shipped_hvac_csvs_load_successfully() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");

        let csv_dirs = [
            defaults_dir.join("hvac_cooling"),
            defaults_dir.join("hvac_heating"),
        ];

        for dir in &csv_dirs {
            let entries = std::fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()));
            for entry in entries {
                let path = entry.unwrap().path();
                if path.extension().is_none_or(|ext| ext != "csv") {
                    continue;
                }

                let set = load_hvac_csv_file(&path).unwrap_or_else(|e| {
                    panic!(
                        "shipped HVAC CSV {} must load successfully: {e}",
                        path.display()
                    )
                });

                // Each variant must have non-zero leading coefficients for
                // both the EIR and capacity biquadratic curves. A zero value
                // here would indicate a parse failure or missing row that
                // silently fell back to 0.0.
                for v in &set.variants {
                    assert!(
                        v.eir_t.coeffs[0] != 0.0,
                        "{}: variant {} has a_eir_t = 0 (zero-fallback suspected)",
                        path.display(),
                        v.name,
                    );
                    assert!(
                        v.cap_t.coeffs[0] != 0.0,
                        "{}: variant {} has a_cap_t = 0 (zero-fallback suspected)",
                        path.display(),
                        v.name,
                    );
                }
            }
        }
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
            r#"# Water heater default parameters
Description,Name,Value,Units
Tank_Volume_50gal_m3,Vol_50gal_m3,0.189,m3
Tank_Volume_50gal_L,Vol_50gal_L,189.3,L
Tank_Volume_80gal_m3,Vol_80gal_m3,0.303,m3
Tank_Volume_80gal_L,Vol_80gal_L,302.8,L
Tank_Height_50gal,H_50gal,1.22,m
UA_50gal_R12,UA_50gal_R12,1.10,W_per_K
UA_80gal_R12,UA_80gal_R12,1.49,W_per_K
UEF_GasStorage_50gal,UEF_GasStorage_50gal,0.64,dimensionless
UEF_ElectricResistance_50gal,UEF_ElecRes_50gal,0.93,dimensionless
UEF_HPWH_50gal,UEF_HPWH_50gal,3.70,dimensionless
HeatingCapacity_GasStorage_50gal,HC_GasStorage_50gal,11170,W
Setpoint,T_set,51.67,degC
Deadband,T_db,5.56,deltaC
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
            (wh.get("Key2").unwrap() - std::f64::consts::PI).abs() < 1e-2,
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
    fn kw_unit_converted_to_w_at_load_time() {
        // Regression: if a CSV row declares P_hw1 as 4.5 kW, the loader must
        // convert to 4500 W internally. Without the ×1000 conversion, a parser
        // that silently assumes W would produce a 1000× error in power values.
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        std::fs::write(
            wh_dir.join("default_paramters.csv"),
            r#"Description,Name,Value,Units
Element_Power_Lower_Tank,P_hw1,4.5,kW
Element_Power_Upper_Tank,P_hw2,4.5,kW
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

        // 4.5 kW × 1000 = 4500 W.
        assert!(
            (wh.get("P_hw1").unwrap() - 4500.0).abs() < 1e-6,
            "P_hw1 4.5 kW should convert to 4500 W, got {}",
            wh.get("P_hw1").unwrap()
        );
        assert!(
            (wh.get("P_hw2").unwrap() - 4500.0).abs() < 1e-6,
            "P_hw2 4.5 kW should convert to 4500 W, got {}",
            wh.get("P_hw2").unwrap()
        );
    }

    #[test]
    fn empty_units_row_loaded_and_flagged_by_invariant_check() {
        // The CSV parser stores rows with empty units but does not convert
        // or fabricate a unit. Rows with empty units are accepted at load
        // time (they are still parsed, value stored as-is). The invariant
        // checker is responsible for flagging empty-units rows at startup.
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let wh_dir = dir.path().join("water_heating");
        std::fs::create_dir_all(&wh_dir).unwrap();
        std::fs::write(
            wh_dir.join("default_paramters.csv"),
            r#"Description,Name,Value,Units
Some_Parameter,P_test,42.0,
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

        // The value is still accessible — loading is non-fatal.
        assert!(
            (wh.get("P_test").unwrap() - 42.0).abs() < 1e-6,
            "empty-units row should still be loaded with value 42.0"
        );
    }

    #[test]
    fn all_known_unit_strings_parse_without_error() {
        // Verify that every unit in KNOWN_WATER_HEATING_UNITS is accepted
        // by the unit conversion function and produces a valid result.
        let test_units = &[
            ("degC", 25.0),
            ("deltaC", 5.0),
            ("K", 300.0),
            ("W", 4500.0),
            ("kW", 4.5), // should convert to 4500 W
            ("m3", 0.189),
            ("L", 189.3),
            ("m", 1.2),
            ("W_per_K", 2.0),
            ("m2_K_per_W", 1.76),
            ("L_per_day", 208.2),
            ("kg_per_s", 0.1262),
            ("dimensionless", 0.92),
            ("C", 51.67), // legacy alias for degC
        ];

        for (unit, raw_value) in test_units {
            let result = convert_water_heating_value_to_si(*raw_value, unit);
            if *unit == "kW" {
                assert!(
                    (result - 4500.0).abs() < 1e-6,
                    "4.5 kW should convert to 4500 W, got {result}"
                );
            }
        }

        // Empty string (CSV parser passes empty string for missing column)
        // is accepted as a valid unit — it means "no unit declared."
        // The invariant checker flags it separately.
        let _ = convert_water_heating_value_to_si(1.0, "");

        // Unrecognized unit passes through with a warning (non-fatal).
        // The invariant checker is responsible for flagging it at startup.
        let _ = convert_water_heating_value_to_si(1.0, "furlongs_per_fortnight");
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
Setpoint,T_set,60.0,degC
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

    // ── Draw schedule integration tests ─────────────────────────────────

    fn load_draw_schedule_csv(path: &Path) -> Vec<(String, f64)> {
        let mut rdr = csv::Reader::from_path(path).expect("CSV must be readable");
        let headers = rdr.headers().expect("CSV must have headers").clone();
        let water_col_idx = headers
            .iter()
            .position(|h| h == "Water Heating (L/min)" || h == "hot_water_fixtures")
            .expect("draw schedule CSV must have a water heating column");
        let time_col_idx = headers
            .iter()
            .position(|h| h == "Time")
            .expect("draw schedule CSV must have a Time column");
        let mut rows: Vec<(String, f64)> = Vec::new();
        for result in rdr.records() {
            let record = result.expect("CSV row must parse");
            let time = record.get(time_col_idx).unwrap_or("").to_string();
            let value: f64 = record
                .get(water_col_idx)
                .unwrap_or("0")
                .parse()
                .unwrap_or(0.0);
            rows.push((time, value));
        }
        rows
    }

    fn hour_from_timestamp(ts: &str) -> Option<u32> {
        ts.split_whitespace()
            .nth(1)?
            .split(':')
            .next()?
            .parse()
            .ok()
    }

    fn day_from_timestamp(ts: &str) -> Option<usize> {
        ts.split_whitespace()
            .next()?
            .split('/')
            .nth(1)?
            .parse::<usize>()
            .ok()
            .map(|d| d - 1)
    }

    #[test]
    fn residential_schedule_integrates_to_240_l_per_day() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let csv_path = defaults_dir
            .join("water_heating")
            .join("WH Medium Residential Schedule.csv");
        assert!(csv_path.exists(), "residential schedule CSV must exist");
        let rows = load_draw_schedule_csv(&csv_path);
        assert!(!rows.is_empty());
        let mut day_totals: Vec<f64> = Vec::new();
        let mut cur_day: Option<usize> = None;
        let mut cur_sum = 0.0;
        for (ts, lpm) in &rows {
            let d = day_from_timestamp(ts).expect("timestamp must parse");
            if cur_day != Some(d) {
                if cur_day.is_some() {
                    day_totals.push(cur_sum);
                }
                cur_sum = 0.0;
                cur_day = Some(d);
            }
            cur_sum += lpm * 60.0;
        }
        if cur_day.is_some() {
            day_totals.push(cur_sum);
        }
        assert!(!day_totals.is_empty());
        for (i, total) in day_totals.iter().enumerate() {
            assert!(
                (total - 240.0).abs() < 5.0,
                "day {} draw {:.1} L, expected ~240 L",
                i + 1,
                total,
            );
        }
    }

    #[test]
    fn residential_schedule_has_morning_and_evening_peaks() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let csv_path = defaults_dir
            .join("water_heating")
            .join("WH Medium Residential Schedule.csv");
        let rows = load_draw_schedule_csv(&csv_path);
        let day_count = day_from_timestamp(&rows.last().unwrap().0).unwrap_or(0) + 1;
        for day_idx in 0..day_count {
            let mut hl = [0.0f64; 24];
            for (ts, lpm) in &rows {
                if day_from_timestamp(ts) != Some(day_idx) {
                    continue;
                }
                if let Some(h) = hour_from_timestamp(ts) {
                    hl[h as usize] += lpm * 60.0;
                }
            }
            let morning_max = hl[6..10].iter().cloned().fold(0.0, f64::max);
            let morning_avg = hl[6..10].iter().sum::<f64>() / 4.0;
            let evening_max = hl[17..22].iter().cloned().fold(0.0, f64::max);
            let other_avg: f64 = (0..24)
                .filter(|h| !(6..10).contains(h) && !(17..22).contains(h))
                .map(|h| hl[h])
                .sum::<f64>()
                / 15.0;
            assert!(
                morning_max > other_avg,
                "day {} morning peak ({:.1} L/h) must exceed non-peak avg ({:.1})",
                day_idx + 1,
                morning_max,
                other_avg,
            );
            let overnight_avg = hl[0..6].iter().sum::<f64>() / 6.0;
            assert!(
                morning_avg > overnight_avg,
                "day {} morning avg ({:.1}) must exceed overnight avg ({:.1})",
                day_idx + 1,
                morning_avg,
                overnight_avg,
            );
            assert!(
                evening_max > 0.0,
                "day {} must have evening draws",
                day_idx + 1
            );
        }
    }

    #[test]
    fn residential_schedule_overnight_draw_below_5_percent() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let csv_path = defaults_dir
            .join("water_heating")
            .join("WH Medium Residential Schedule.csv");
        let rows = load_draw_schedule_csv(&csv_path);
        let day_count = day_from_timestamp(&rows.last().unwrap().0).unwrap_or(0) + 1;
        for day_idx in 0..day_count {
            let mut hl = [0.0f64; 24];
            for (ts, lpm) in &rows {
                if day_from_timestamp(ts) != Some(day_idx) {
                    continue;
                }
                if let Some(h) = hour_from_timestamp(ts) {
                    hl[h as usize] += lpm * 60.0;
                }
            }
            let total: f64 = hl.iter().sum();
            let overnight = hl[22..24].iter().sum::<f64>() + hl[0..5].iter().sum::<f64>();
            let pct = 100.0 * overnight / total;
            assert!(
                pct < 5.0,
                "day {} overnight draw (22-5) is {:.1}%, must be <5%",
                day_idx + 1,
                pct,
            );
        }
    }

    #[test]
    fn uef_test_schedule_integrates_to_208_2_l_per_day() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let csv_path = defaults_dir
            .join("water_heating")
            .join("WH UEF Test Medium Draw Profile.csv");
        assert!(csv_path.exists(), "UEF test schedule must exist");
        let rows = load_draw_schedule_csv(&csv_path);
        assert!(!rows.is_empty());
        let mut day_totals: Vec<f64> = Vec::new();
        let mut cur_day: Option<usize> = None;
        let mut cur_sum = 0.0;
        for (ts, lpm) in &rows {
            let d = day_from_timestamp(ts).expect("timestamp must parse");
            if cur_day != Some(d) {
                if cur_day.is_some() {
                    day_totals.push(cur_sum);
                }
                cur_sum = 0.0;
                cur_day = Some(d);
            }
            cur_sum += lpm;
        }
        if cur_day.is_some() {
            day_totals.push(cur_sum);
        }
        let expected = 208.2;
        for (i, total) in day_totals.iter().enumerate() {
            assert!(
                (total - expected).abs() < 1.0,
                "UEF test day {} draw {:.1} L, expected {:.1} L",
                i + 1,
                total,
                expected,
            );
        }
    }

    #[test]
    fn ev_defaults_directory_documents_file_schemas() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let ev_dir = defaults_dir.join("ev");
        assert!(ev_dir.exists(), "defaults/ev/ directory must exist");

        let has_readme = ev_dir.join("README.md").exists();
        let has_vehicle_specs = ev_dir.join("vehicle_specs.csv").exists();
        assert!(
            has_readme || has_vehicle_specs,
            "defaults/ev/ must contain either README.md or vehicle_specs.csv \
             documenting file schemas and vehicle-to-type relationships"
        );
    }

    // ── EV vehicle mapping tests ────────────────────────────────────────

    /// Write a minimal 50-entry vehicle mapping CSV.
    fn write_vehicle_mapping_csv(dir: &Path) {
        let mut csv_content = String::from(
            "profile_column,vehicle_type,profile_file,capacity_kwh,charger_power_kw,efficiency\n",
        );
        for i in 1..=35 {
            csv_content.push_str(&format!(
                "Vehicle {i},MY2030_BEV_SUV,pdf_Veh{},117.6,10.26,0.9\n",
                ((i - 1) % 4) + 1
            ));
        }
        for i in 36..=50 {
            csv_content.push_str(&format!(
                "Vehicle {i},MY2030_PHEV_SUV,pdf_Veh{},14.8,7.2,0.9\n",
                ((i - 1) % 4) + 1
            ));
        }
        std::fs::write(dir.join("vehicle_mapping.csv"), csv_content).unwrap();
    }

    #[test]
    fn vehicle_mapping_csv_parses_successfully() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let ev_dir = dir.path().join("ev");
        std::fs::create_dir_all(&ev_dir).unwrap();
        write_vehicle_mapping_csv(&ev_dir);
        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert!(store.has_ev_mapping());

        let mapping = store.ev_mapping().unwrap();
        assert_eq!(mapping.count(), 50);

        // Verify a BEV entry.
        let veh1 = mapping.get("Vehicle 1").expect("Vehicle 1 should exist");
        assert_eq!(veh1.vehicle_type, "MY2030_BEV_SUV");
        assert!((veh1.capacity_kwh - 117.6).abs() < 1e-6);
        assert!((veh1.charger_power_kw - 10.26).abs() < 1e-6);
        assert!((veh1.efficiency - 0.9).abs() < 1e-6);

        // Verify a PHEV entry.
        let veh50 = mapping.get("Vehicle 50").expect("Vehicle 50 should exist");
        assert_eq!(veh50.vehicle_type, "MY2030_PHEV_SUV");
        assert!((veh50.capacity_kwh - 14.8).abs() < 1e-6);
        assert!((veh50.charger_power_kw - 7.2).abs() < 1e-6);
    }

    #[test]
    fn vehicle_mapping_returns_none_for_missing_vehicle() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let ev_dir = dir.path().join("ev");
        std::fs::create_dir_all(&ev_dir).unwrap();
        write_vehicle_mapping_csv(&ev_dir);
        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        let mapping = store.ev_mapping().unwrap();

        assert!(
            mapping.get("Vehicle 999").is_none(),
            "nonexistent vehicle column should return None"
        );
        assert!(
            mapping.get("").is_none(),
            "empty column name should return None"
        );
    }

    #[test]
    fn vehicle_mapping_csv_missing_file_is_non_fatal() {
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

        // No vehicle_mapping.csv in ev/ — should load fine.
        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert!(!store.has_ev_mapping(), "no CSV → no EV mapping");
        assert!(store.ev_mapping().is_none());
    }

    #[test]
    fn vehicle_mapping_with_fewer_than_50_entries_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let ev_dir = dir.path().join("ev");
        std::fs::create_dir_all(&ev_dir).unwrap();
        std::fs::write(
            ev_dir.join("vehicle_mapping.csv"),
            "profile_column,vehicle_type,profile_file,capacity_kwh,charger_power_kw,efficiency\n\
             Vehicle 1,MY2030_BEV_SUV,pdf_Veh1,117.6,10.26,0.9\n\
             Vehicle 2,MY2030_PHEV_SUV,pdf_Veh2,14.8,7.2,0.9\n",
        )
        .unwrap();
        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        assert!(
            !store.has_ev_mapping(),
            "incomplete mapping (2 entries) should not be exposed"
        );
        assert!(store.ev_mapping().is_none());
    }

    #[test]
    fn vehicle_mapping_csv_malformed_rows_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        let ev_dir = dir.path().join("ev");
        std::fs::create_dir_all(&ev_dir).unwrap();

        // Build 50 rows: one for each Vehicle, but Vehicle 25 and Vehicle 26
        // have unparseable numeric fields.
        let mut csv_content = String::from(
            "profile_column,vehicle_type,profile_file,capacity_kwh,charger_power_kw,efficiency\n",
        );
        for i in 1..=50 {
            if i == 25 {
                csv_content.push_str("Vehicle 25,MY2030_BEV_SUV,pdf_Veh1,not_a_number,10.26,0.9\n");
            } else if i == 26 {
                csv_content.push_str("Vehicle 26,MY2030_BEV_SUV,pdf_Veh2,117.6,also_bad,0.9\n");
            } else {
                let vtype = if i >= 36 {
                    "MY2030_PHEV_SUV"
                } else {
                    "MY2030_BEV_SUV"
                };
                let cap = if i >= 36 { "14.8" } else { "117.6" };
                let chg = if i >= 36 { "7.2" } else { "10.26" };
                csv_content.push_str(&format!(
                    "Vehicle {i},{vtype},pdf_Veh{},{cap},{chg},0.9\n",
                    ((i - 1) % 4) + 1
                ));
            }
        }
        std::fs::write(ev_dir.join("vehicle_mapping.csv"), csv_content).unwrap();
        create_subdirs(
            dir.path(),
            &[
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "generator",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store = DefaultsStore::load(dir.path()).expect("load defaults");
        // 2 malformed rows skipped → 48 valid entries → fewer than 50 → None.
        assert!(store.ev_mapping().is_none());
    }

    #[test]
    fn real_vehicle_mapping_csv_loads_from_fixture() {
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("defaults");
        let store = DefaultsStore::load(&defaults_dir).expect("load defaults");

        assert!(
            store.has_ev_mapping(),
            "real defaults/ev/vehicle_mapping.csv should be present and parseable"
        );

        let mapping = store.ev_mapping().unwrap();
        assert_eq!(
            mapping.count(),
            50,
            "should have 50 entries, one per Vehicle 1–50"
        );

        // Verify all 50 expected vehicle columns are present.
        for i in 1..=50 {
            let col_name = format!("Vehicle {i}");
            let entry = mapping.get(&col_name);
            assert!(
                entry.is_some(),
                "Vehicle {i} should have a mapping entry, but was not found"
            );
            let e = entry.unwrap();
            assert!(
                !e.vehicle_type.is_empty(),
                "Vehicle {i} vehicle_type must not be empty"
            );
            assert!(
                !e.profile_file.is_empty(),
                "Vehicle {i} profile_file must not be empty"
            );
            assert!(
                e.capacity_kwh > 0.0,
                "Vehicle {i} capacity_kwh must be positive"
            );
            assert!(
                e.charger_power_kw > 0.0,
                "Vehicle {i} charger_power_kw must be positive"
            );
            assert!(
                e.efficiency > 0.0 && e.efficiency <= 1.0,
                "Vehicle {i} efficiency must be in (0.0, 1.0]"
            );
        }

        // Verify vehicle type distribution.
        let bev_count = mapping
            .iter()
            .filter(|e| e.vehicle_type == "MY2030_BEV_SUV")
            .count();
        let phev_count = mapping
            .iter()
            .filter(|e| e.vehicle_type == "MY2030_PHEV_SUV")
            .count();
        assert_eq!(bev_count + phev_count, 50);
        assert!(bev_count > 0, "should have at least one BEV");
        assert!(phev_count > 0, "should have at least one PHEV");

        // Verify parameter consistency within each vehicle type.
        for entry in mapping.iter() {
            if entry.vehicle_type == "MY2030_BEV_SUV" {
                assert!(
                    (entry.capacity_kwh - 117.6).abs() < 0.01,
                    "All BEV entries must have capacity 117.6 kWh, got {}",
                    entry.capacity_kwh
                );
                assert!(
                    (entry.charger_power_kw - 10.26).abs() < 0.01,
                    "All BEV entries must have charger_power_kw 10.26, got {}",
                    entry.charger_power_kw
                );
            } else if entry.vehicle_type == "MY2030_PHEV_SUV" {
                assert!(
                    (entry.capacity_kwh - 14.8).abs() < 0.01,
                    "All PHEV entries must have capacity 14.8 kWh, got {}",
                    entry.capacity_kwh
                );
                assert!(
                    (entry.charger_power_kw - 7.2).abs() < 0.01,
                    "All PHEV entries must have charger_power_kw 7.2, got {}",
                    entry.charger_power_kw
                );
            }
        }
    }

    #[test]
    fn load_generator_efficiency_curve_from_toml_populates_six_points() {
        let dir = tempfile::tempdir().unwrap();
        create_subdirs(
            dir.path(),
            &[
                "generator",
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "loads",
                "pv",
                "water_heating",
            ],
        );
        write_minimal_zip(dir.path());

        // Write the 6-point efficiency curve TOML.
        let toml_content = r#"
[[points]]
capacity_ratio = 0.0
efficiency_ratio = 0.0

[[points]]
capacity_ratio = 0.1
efficiency_ratio = 0.47

[[points]]
capacity_ratio = 0.167
efficiency_ratio = 0.62

[[points]]
capacity_ratio = 0.333
efficiency_ratio = 0.78

[[points]]
capacity_ratio = 0.666
efficiency_ratio = 0.94

[[points]]
capacity_ratio = 1.0
efficiency_ratio = 1.0
"#;
        std::fs::write(
            dir.path().join("generator").join("efficiency_curve.toml"),
            toml_content,
        )
        .unwrap();

        let store = DefaultsStore::load(dir.path()).expect("load defaults");

        let points = store
            .generator_efficiency_curve_points()
            .expect("generator curve should be loaded");
        assert_eq!(points.len(), 6, "should have 6 curve points");
        assert!((points[0].capacity_ratio - 0.0).abs() < 1e-12);
        assert!((points[0].efficiency_ratio - 0.0).abs() < 1e-12);
        assert!((points[1].capacity_ratio - 0.1).abs() < 1e-12);
        assert!((points[1].efficiency_ratio - 0.47).abs() < 1e-12);
        assert!((points[5].capacity_ratio - 1.0).abs() < 1e-12);
        assert!((points[5].efficiency_ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn missing_generator_efficiency_curve_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        create_subdirs(
            dir.path(),
            &[
                "generator",
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "loads",
                "pv",
                "water_heating",
            ],
        );
        write_minimal_zip(dir.path());
        // No efficiency_curve.toml in generator/ — simulate missing file.

        let store = DefaultsStore::load(dir.path()).expect("load defaults");

        assert!(
            store.generator_efficiency_curve_points().is_none(),
            "missing efficiency_curve.toml should return None"
        );
    }

    #[test]
    fn malformed_generator_efficiency_curve_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        create_subdirs(
            dir.path(),
            &[
                "generator",
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "loads",
                "pv",
                "water_heating",
            ],
        );
        write_minimal_zip(dir.path());

        // Write a malformed TOML (reversed capacity_ratio order — not strictly increasing).
        let toml_content = r#"
[[points]]
capacity_ratio = 1.0
efficiency_ratio = 1.0

[[points]]
capacity_ratio = 0.0
efficiency_ratio = 0.0
"#;
        std::fs::write(
            dir.path().join("generator").join("efficiency_curve.toml"),
            toml_content,
        )
        .unwrap();

        let store = DefaultsStore::load(dir.path()).expect("load defaults");

        assert!(
            store.generator_efficiency_curve_points().is_none(),
            "non-strictly-increasing curve points should be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // CSV header invariant tests
    // -----------------------------------------------------------------------

    #[test]
    fn generator_dir_loads_cleanly_without_battery_csv() {
        // Verify that the defaults loader does not attempt to parse
        // battery-specific fields for a generator equipment type when
        // the generator directory is clean.
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        create_subdirs(
            dir.path(),
            &[
                "generator",
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let store =
            DefaultsStore::load(dir.path()).expect("load defaults from clean generator dir");

        // Generator defaults are keyed under ["default-parameters"] by
        // load_toml_dir via normalize_equipment_key (which converts
        // "Default Parameters" to "default-parameters"). With no TOML files
        // in the generator directory, there should be no entries.
        let defaults = store.equipment_defaults(DefaultsCategory::Generator, "default-parameters");
        assert!(
            defaults.is_none(),
            "generator defaults should be empty when no TOML files present"
        );
    }

    #[test]
    fn generator_dir_ignores_battery_csv_with_name_column() {
        // OCHRE-format CSV (Description,Name,Value,Units header) with battery
        // parameter names placed in the generator directory. The loader must
        // succeed gracefully (CSV is ignored by load_toml_dir), and the
        // invariant check must detect the misplaced parameters.
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        create_subdirs(
            dir.path(),
            &[
                "generator",
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let gen_dir = dir.path().join("generator");
        // Write a CSV file with battery-specific parameter names in the Name
        // column — this exactly replicates the original bug.
        std::fs::write(
            gen_dir.join("stale_battery_params.csv"),
            "Description,Name,Value,Units\n\
             Battery Capacity,capacity_kwh,13.5,kWh\n\
             Max Power,capacity,5.0,kW\n\
             Initial SOC,soc_init,0.5,fraction\n\
             Charge Efficiency,efficiency_charge,0.98,fraction\n",
        )
        .unwrap();

        // Loading must succeed — CSV files are not consumed by the loader.
        let store =
            DefaultsStore::load(dir.path()).expect("load defaults with misplaced battery CSV");

        // Generator EquipmentDefaults should be empty — the CSV was not parsed.
        let defaults = store.equipment_defaults(DefaultsCategory::Generator, "default-parameters");
        assert!(
            defaults.is_none(),
            "CSV should not leak into generator defaults map"
        );

        // The invariant check function also verifies: call it directly to
        // confirm it detects the foreign parameters.
        let findings = check_dir_for_foreign_csv_params(&gen_dir, BATTERY_ONLY_FOR_TEST);
        assert!(
            !findings.is_empty(),
            "invariant check should detect battery params in generator CSV"
        );
        let first = &findings[0];
        assert!(
            first.contains("capacity_kwh")
                || first.contains("soc_init")
                || first.contains("efficiency_charge"),
            "finding should name the specific battery parameters detected, got: {first}"
        );
    }

    #[test]
    fn generator_dir_ignores_csv_with_battery_header_columns() {
        // Wide-format CSV where header columns are the parameter names
        // (no Description/Name/Value/Units structure). Battery-specific
        // header columns in a generator CSV should be detected.
        let dir = tempfile::tempdir().unwrap();
        write_minimal_zip(dir.path());
        create_subdirs(
            dir.path(),
            &[
                "generator",
                "hvac_cooling",
                "hvac_heating",
                "battery",
                "envelope",
                "ev",
                "loads",
                "pv",
                "water_heating",
            ],
        );

        let gen_dir = dir.path().join("generator");
        // Wide format with battery-specific column headers — no "Name" column.
        std::fs::write(
            gen_dir.join("battery_wide_format.csv"),
            "capacity_kwh,soc_init,soc_max,efficiency_charge,discharge_pct\n\
             13.5,0.5,0.95,0.98,0.0\n",
        )
        .unwrap();

        let store = DefaultsStore::load(dir.path())
            .expect("load defaults with battery CSV in generator dir");

        // Loader must succeed.
        let defaults = store.equipment_defaults(DefaultsCategory::Generator, "default-parameters");
        assert!(
            defaults.is_none(),
            "wide-format CSV should not leak into generator defaults"
        );

        // Direct invariant check: headers should be detected as foreign.
        let findings = check_dir_for_foreign_csv_params(&gen_dir, BATTERY_ONLY_FOR_TEST);
        assert!(
            !findings.is_empty(),
            "wide-format battery CSV should be detected by header-level check"
        );
        let first = &findings[0];
        assert!(
            first.contains("capacity_kwh") || first.contains("soc_init"),
            "finding should name the battery column headers, got: {first}"
        );
    }

    /// Test-only copy of the battery-only parameter list so the invariant
    /// function can be unit-tested without duplicating the production constant.
    #[cfg(test)]
    const BATTERY_ONLY_FOR_TEST: &[&str] = &[
        "capacity_kwh",
        "soc_init",
        "soc_max",
        "soc_min",
        "efficiency_charge",
        "efficiency_discharge",
        "efficiency_inverter",
        "discharge_pct",
        "initial_voltage",
        "v_cell",
        "ah_cell",
        "r_cell",
        "charge_start_hour",
        "discharge_start_hour",
        "charge_power",
        "discharge_power",
        "charge_from_solar",
        "import_limit",
        "export_limit",
        "thermal_r",
        "thermal_c",
    ];
}
