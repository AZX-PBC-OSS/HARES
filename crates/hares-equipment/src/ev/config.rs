use hares_types::HaresError;
use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

use hares_types::{ChargingLevel, ChargingPriority, ChargingStrategy, PlugInPolicy};

pub(super) const DEFAULT_FUEL_ECONOMY_KWH_PER_MI: f64 = 0.325;
/// The default fuel economy in SI (J/m), the `uom` expression of the
/// same value (0.3 kWh/mi is 671.08 J/m to the printed digits; the
/// default is 0.325).
pub(super) const DEFAULT_FUEL_ECONOMY_J_PER_M: f64 =
    DEFAULT_FUEL_ECONOMY_KWH_PER_MI * 3.6e6 / 1609.344;
/// The same hardware bounds in W (SI), the clamp the config's
/// `max_charging_power_w` runs against.
pub(super) const L1_MIN_POWER_W: f64 = 1000.0;
pub(super) const L1_MAX_POWER_W: f64 = 1800.0;
pub(super) const L2_MIN_POWER_W: f64 = 3600.0;
pub(super) const L2_MAX_POWER_W: f64 = 11500.0;
/// The placeholder constructor's Level 2 rate: the top of the OCHRE
/// vehicle-number table, the deleted raw default rule's value for the
/// default capacity. `init_typed` replaces it from the config.
pub(super) const DEFAULT_MAX_CHARGING_POWER_KW: f64 = 11.5;
/// OCHRE EV.py: default 75 kWh EV battery capacity (vehicle number 4
/// class, the BEV ≥ 175 mi range tier).
pub(super) const DEFAULT_CAPACITY_KWH: f64 = 75.0;
pub(super) const DEFAULT_SOC: f64 = 1.0;
pub(super) const DEFAULT_SOC_MAX: f64 = 1.0;
/// OCHRE EV.py:11 — EV_EFFICIENCY = 0.9, the AC→DC onboard charger
/// efficiency (a single figure covering both directions, unlike the
/// stationary Battery's two-field split).
pub(super) const DEFAULT_EFFICIENCY: f64 = 0.9;
/// SAE J1772 Level 1 supply voltage: 120 V single-phase.
pub(super) const DEFAULT_L1_VOLTAGE_V: f64 = 120.0;
/// Li-ion lithium plating occurs below 0 °C: metallic lithium plates on
/// the anode instead of intercalating, and the damage is permanent — so
/// BMS low-temperature charge cutoffs at 0 °C are standard practice
/// (Battery University BU-410; corroborated by the stationary Battery's
/// DEFAULT_MIN_CHARGE_TEMP_C, battery/mod.rs, "all manufacturers block
/// charging here"). Real physics the model keeps: what real vehicles pair
/// it with — and HARES now models — is pack preconditioning while plugged
/// in (the heater), not a lower cutoff.
pub(super) const DEFAULT_MIN_CHARGE_TEMP_C: f64 = 0.0;
/// Full charge power above 10 °C: Tesla and FranklinWH both require ~10 °C
/// for full charge/discharge power (stationary Battery precedent,
/// battery/mod.rs DEFAULT_FULL_POWER_TEMP_C comment). Linear derate
/// between `min_charge_temp_c` and this value.
pub(super) const DEFAULT_FULL_POWER_TEMP_C: f64 = 10.0;
/// Pack preheating heater power: 5 kW, the entry class of the HV PTC
/// coolant-heater family used for BEV/PHEV battery conditioning (Webasto
/// High Voltage Heater product line: 5/7/10/12 kW heat-output classes for
/// battery-electric thermal management incl. battery conditioning,
/// webasto.com HVH datasheet; trade range for pack preheating duty is
/// 3–8 kW — distinct from the separate 1.5–3 kW cabin duty). Constant-
/// power model (real PTC elements self-regulate to ~12 % lower average
/// draw; the error direction is known and conservative for the grid-load
/// studies the billed draw feeds — documented divergence).
pub(super) const DEFAULT_HEATER_POWER_W: f64 = 5000.0;
/// Heater activation threshold: 5 °C. Li-ion cell datasheets specify low
/// charge-temperature limits in the 0–10 °C band (Better Battery Design,
/// "Charging at low Temperatures", 2021: "low charge limits somewhere
/// between 10 °C and 0 °C being verboten"); industry guidance keeps charge
/// temperatures above ~5 °C for cell health, not merely above the 0 °C
/// plating floor. 5 °C is the band midpoint and matches the stationary
/// Battery catalog's Franklin-class heater_threshold_c entries
/// (battery/catalog.rs: 0.0 and 5.0).
pub(super) const DEFAULT_HEATER_THRESHOLD_C: f64 = 5.0;
/// Series cell count: 96S ≈ 350–360 V nominal NMC pack, the prevailing
/// EV architecture (Tesla Model 3/Model Y 96S; Chevrolet Bolt 96S3P).
pub(super) const DEFAULT_N_SERIES: u32 = 96;
/// Cell capacity used to derive `n_parallel` from the capacity (kWh) when the
/// topology is not explicitly configured: the 21700-format NMC cell class
/// (LG M50: 5.0 Ah nominal).
pub(super) const DEFAULT_CELL_AH: f64 = 5.0;
/// Cell nominal voltage used in the `n_parallel` derivation: the Smith
/// 2017 reference OCV (NREL/CP-5400-67102, §II reference constants:
/// Vref = 3.7 V).
const CELL_NOMINAL_V: f64 = 3.7;
/// Cell internal resistance at mid-SOC, 25 °C — the same cited default
/// the stationary Battery carries (battery/mod.rs
/// DEFAULT_CELL_RESISTANCE_OHM: "typical 18650/21700 Li-NMC cell"), so
/// both packs share one parameterization and one I²R home.
pub(super) const DEFAULT_CELL_RESISTANCE_OHM: f64 = 0.005;
/// Pack mass per rated kWh: Tesla Model 3 Long Range pack ≈ 479 kg for
/// 75 kWh (pack spec listing, evshop.eu; consistent with the 250–600 kg
/// industry range for this class). Used with the cell specific heat to
/// derive the pack thermal mass — the physics route (a flat per-model
/// constant cannot scale across pack sizes and carries no citation).
pub(super) const DEFAULT_PACK_MASS_KG_PER_KWH: f64 = 6.4;
/// Li-ion cell specific heat: 1000 J/(kg·K), the representative midpoint
/// of the 800–1100 J/(kg·K) calorimetry literature band (NASA TFAWS-18
/// battery thermal characterization; batterydesign.net calorimetry
/// summary). Consistent with the stationary Battery's density (OCHRE
/// Battery.py 90 kJ/K per 13.5 kWh = 6.7 kJ/K per kWh — cross-check, not
/// the source).
pub(super) const DEFAULT_CELL_SPECIFIC_HEAT_J_PER_KG_K: f64 = 1000.0;
/// Base UA at the reference pack size: the stationary Battery's 5.0 W/K
/// enclosed-pack default (battery/mod.rs DEFAULT_CELL_UA_W_PER_K).
const UA_BASE_W_PER_K: f64 = 5.0;
/// Reference pack size for the UA area scaling: the stationary Battery's
/// 13.5 kWh default (OCHRE Battery.py).
const UA_REFERENCE_CAPACITY_KWH: f64 = 13.5;
// Standard laboratory reference temperature (20 °C / 293.15 K). True
// fallback ONLY where ambient is genuinely unavailable at construction
// time (pre-init telemetry placeholder): `init` resolves the pack
// temperature by cascade — explicit config, else outdoor ambient (the
// EV carries no zone). This is a conventional engineering default, not a
// standard mandate. No ASHRAE or SAE standard specifies an EV battery
// initialisation temperature for simulation.
pub(super) const DEFAULT_BATTERY_TEMP_C: f64 = 20.0;
/// HARES engineering default (no standard specifies V2L reserve floors;
/// OCHRE has no V2L model for EVs): keep 20 % of the pack for mobility
/// when backing up loads.
pub(super) const DEFAULT_V2L_SOC_RESERVE: f64 = 0.2;
/// HARES engineering default: 3 kW V2L discharge ceiling, sized to backup
/// circuits (refrigeration + lighting + essentials) on a 120/240 V
/// residential split phase.
pub(super) const DEFAULT_V2L_MAX_DISCHARGE_KW: f64 = 3.0;
/// HARES engineering default: grid-service discharge keeps 30 % of the
/// pack (stricter than V2L — the departure guarantee must survive a
/// V2G session).
pub(super) const DEFAULT_V2G_SOC_RESERVE: f64 = 0.3;
/// HARES engineering default: 5 kW V2G ceiling, the common residential
/// bidirectional EVSE rating class.
pub(super) const DEFAULT_V2G_MAX_DISCHARGE_KW: f64 = 5.0;
/// SOC above which CC-CV tapering of charging power begins.
///
/// This is a conventional engineering default for NMC Li‑ion chemistry: the
/// CC (constant‑current) region delivers near‑constant power through ~80‑90%
/// SOC, then the CV (constant‑voltage) region tapers power as the battery
/// approaches full charge.  No single standard specifies exact CC→CV
/// transition SOC and minimum multiplier for generic simulation — these
/// values represent the centroid of commonly‑observed NMC charge behaviour
/// and should be tuned from cell datasheets when available.
///
/// When a charging-curve LUT is present, this parameter is unused — the LUT
/// already captures the electrochemical power roll-off at high SOC.
pub(super) const DEFAULT_CC_CV_TRANSITION_SOC: f64 = 0.85;
pub(super) use hares_physics::constants::SECONDS_PER_HOUR;
pub(super) const MIN_TIMESTEP_HOURS: f64 = 1e-9;

/// Default pack thermal mass [J/K], derived from rated capacity through
/// pack mass × cell specific heat:
/// `C = capacity × 6.4 kg/kWh × 1000 J/(kg·K)` (capacity in kWh) → ≈480 kJ/K for a
/// 75 kWh pack. With the default UA this gives a day-scale relaxation
/// time constant (τ = C/UA ≈ 8.5 h at 75 kWh), so a pack does not
/// flash-freeze at sundown — a flat ~20 kJ/K default would give τ ≈ 1.4 h
/// and turn every sub-freezing night into a full-window charging blackout
/// (the pack hits the plating cutoff before the window opens and stays
/// there all night).
pub(super) fn default_thermal_mass_j_per_k(capacity_kwh: f64) -> f64 {
    (capacity_kwh.max(0.0) * DEFAULT_PACK_MASS_KG_PER_KWH * DEFAULT_CELL_SPECIFIC_HEAT_J_PER_KG_K)
        .max(1.0)
}

/// Default pack-to-ambient UA [W/K], area-scaled from the stationary
/// Battery's enclosed-pack default: `UA = 5.0 W/K × (capacity/13.5)^(2/3)`
/// — surface area grows with pack volume, i.e. with mass^(2/3), and the
/// Battery's 5.0 W/K at 13.5 kWh is the in-tree anchor for the
/// parked/plugged-in still-air regime (the regime every charging-physics
/// assertion exercises; road-speed convective boost during the drive is
/// deliberately not modeled — conservative for the cold-charge problem).
/// At 75 kWh this gives 15.7 W/K (τ = C/UA ≈ 8.5 h). A two-sided
/// constraint, stated so it is met as a constraint: the parked-pack
/// regression (20 °C pack, −7 °C night, 8 h, must stay above 0 °C)
/// implies a UA ceiling ≈24 W/K at ~480 kJ/K mass — the (mass, UA) pair
/// is re-derived jointly if a cited derivation exceeds it; the test is
/// never widened.
pub(super) fn default_ua_w_per_k(capacity_kwh: f64) -> f64 {
    UA_BASE_W_PER_K * (capacity_kwh.max(0.0) / UA_REFERENCE_CAPACITY_KWH).powf(2.0 / 3.0)
}

/// Default parallel string count, derived from rated capacity so the
/// topology is capacity-consistent: `n_parallel = ceil(pack_Ah /
/// cell_Ah)` with `pack_Ah = capacity_kwh × 1000 / (n_series × 3.7 V)`.
/// For 75 kWh: 96S43P (implied capacity 76.6 kWh, +2.1 % — the ceil
/// rounds up, never below the declared capacity).
pub(super) fn default_n_parallel(capacity_kwh: f64) -> u32 {
    let pack_ah = capacity_kwh.max(0.0) * 1000.0
        / (DEFAULT_N_SERIES as f64 * CELL_NOMINAL_V).max(f64::EPSILON);
    if pack_ah <= 0.0 {
        return 1;
    }
    let n = (pack_ah / DEFAULT_CELL_AH).ceil() as u32;
    n.max(1)
}

pub(super) fn parse_charging_strategy(s: &str) -> Result<ChargingStrategy, HaresError> {
    let strategy: ChargingStrategy = serde_json::from_str(s)
        .map_err(|e| HaresError::Equipment(format!("invalid charging_strategy: {e}")))?;
    strategy
        .validate()
        .map_err(|e| HaresError::Equipment(format!("invalid charging_strategy: {e}")))?;
    Ok(strategy)
}

/// Parse a `plug_in_policy` override JSON string; see
/// [`parse_charging_strategy`] for the error contract.
pub(super) fn parse_plug_in_policy(s: &str) -> Result<PlugInPolicy, HaresError> {
    let policy: PlugInPolicy = serde_json::from_str(s)
        .map_err(|e| HaresError::Equipment(format!("invalid plug_in_policy: {e}")))?;
    policy
        .validate()
        .map_err(|e| HaresError::Equipment(format!("invalid plug_in_policy: {e}")))?;
    Ok(policy)
}

/// Parse a charging level name into [`ChargingLevel`].
///
/// Accepts the spellings seen across config sources, matched via
/// [`normalize_config_name`](crate::config::normalize_config_name)
/// (case/whitespace/underscore-insensitive): "L1", "Level 1", "1" and
/// "L2", "Level 2", "2" (HPXML supplies "Level 2" or the bare digit;
/// the python surface supplies "L1"/"L2"). Any other value is a
/// configuration error: the charging level sizes the EVSE power bounds, so
/// an unrecognised string silently becoming L2 would silently clamp the
/// configured charge power.
pub(super) fn parse_charging_level(s: &str) -> Result<ChargingLevel, HaresError> {
    match crate::config::normalize_config_name(s).as_str() {
        "l1" | "level1" | "1" => Ok(ChargingLevel::L1),
        "l2" | "level2" | "2" => Ok(ChargingLevel::L2),
        _ => Err(HaresError::Equipment(format!(
            "invalid charging_level '{s}': expected one of L1, Level 1, 1, L2, Level 2, 2 \
             (case/whitespace/underscore-insensitive)"
        ))),
    }
}

/// Typed configuration for an electric vehicle charger.
///
/// All power values are in W, energy in J, temperatures in °C (SI): every
/// quantity enters converted once at its input boundary.
/// `charging_efficiency` is the AC→DC onboard charger efficiency as a fraction in (0, 1].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvConfig {
    /// Equipment instance identifier.
    pub equipment_id: Option<u32>,
    /// Battery pack capacity (J). Resolvers map the HPXML battery capacity to
    /// this field.
    pub capacity_j: f64,
    /// EVSE charging level: "L1" or "L2".
    pub charging_level: Option<String>,
    /// Maximum charging power (W). Clamped to level-appropriate bounds.
    pub max_charging_power_w: f64,
    /// AC→DC onboard charger efficiency [0, 1].
    pub charging_efficiency: Option<f64>,
    /// L1 circuit current (A). Only used when charging_level = "L1".
    pub l1_current_a: Option<f64>,
    /// L1 circuit voltage (V). Defaults to 120 V.
    pub l1_voltage_v: Option<f64>,
    /// Maximum state-of-charge for regular charging [0, 1].
    pub soc_max: Option<f64>,
    /// Initial SOC [0, 1].
    pub initial_soc: Option<f64>,
    /// Initial battery temperature (°C). Defaults to DEFAULT_BATTERY_TEMP_C (20.0 °C).
    pub battery_temp_c: Option<f64>,
    /// Minimum temperature for charging (°C).
    pub min_charge_temp_c: Option<f64>,
    /// Temperature above which full charge power is available (°C).
    pub full_power_temp_c: Option<f64>,
    /// Battery heater power (W). Defaults to the pack-preheating duty
    /// class (see `DEFAULT_HEATER_POWER_W`).
    pub heater_power_w: Option<f64>,
    /// Temperature threshold to activate battery heater (°C).
    ///
    /// Deliberately **not** validated against `min_charge_temp_c`: a
    /// threshold below the plating cutoff is a coherent protect-only
    /// policy — anti-freeze heating without cold-charge preconditioning
    /// — and rejecting it would break a legitimate configuration. The
    /// consequence is observable, not silent: below the cutoff the heater
    /// cycles (heater telemetry, pack temperature rising) while charge
    /// power stays zero until ambient or the heater lifts the pack above
    /// the cutoff; in sustained cold with the pack starting below the
    /// threshold's ceiling, charging waits for the ambient diurnal cycle
    /// (a protect-only vehicle intentionally trades charge availability
    /// for cell protection).
    pub heater_threshold_c: Option<f64>,
    /// Battery pack thermal mass (J/K). Defaults to the capacity-derived
    /// value (pack mass × cell specific heat; see
    /// `default_thermal_mass_j_per_k`).
    pub thermal_mass_j_per_k: Option<f64>,
    /// Pack-to-ambient heat transfer coefficient (W/K). Defaults to the
    /// area-scaled value (see `default_ua_w_per_k`).
    pub ua_w_per_k: Option<f64>,
    /// Series cell count (sets pack voltage). Defaults to 96 (≈350–360 V
    /// nominal NMC, the prevailing EV architecture).
    pub n_series: Option<u32>,
    /// Parallel string count (sets pack current sharing and resistance).
    /// Defaults to a value derived from `capacity_kwh` (see
    /// `default_n_parallel`).
    pub n_parallel: Option<u32>,
    /// Cell internal resistance (Ω) at mid-SOC, 25 °C. Drives the I²R
    /// pack heating during charge, discharge, and drive. Defaults to the
    /// stationary Battery's 5 mΩ cell default so both packs share one
    /// parameterization.
    pub cell_resistance_ohm: Option<f64>,
    /// Enable vehicle-to-load (V2L) discharge.
    pub v2l_enabled: Option<bool>,
    /// Minimum SOC to retain for V2L [0, 1].
    pub v2l_soc_reserve: Option<f64>,
    /// Maximum V2L discharge power (W).
    pub v2l_max_discharge_w: Option<f64>,
    /// Enable vehicle-to-grid (V2G) discharge.
    pub v2g_enabled: Option<bool>,
    /// Minimum SOC to retain for V2G [0, 1].
    pub v2g_soc_reserve: Option<f64>,
    /// Maximum V2G discharge power (W).
    pub v2g_max_discharge_w: Option<f64>,
    /// Battery chemistry string, e.g. "NMC".
    pub chemistry: Option<String>,
    /// Fuel economy (J/m) for range estimation.
    pub fuel_economy_j_per_m: Option<f64>,
    /// SOC target when the vehicle must be ready.
    pub ready_soc: Option<f64>,
    /// Charging strategy JSON blob.
    pub charging_strategy: Option<String>,
    /// Plug-in policy JSON blob.
    pub plug_in_policy: Option<String>,
    /// Hard power limit from external source (W).
    pub power_limit_w: Option<f64>,
    /// Initial connection state string.
    pub initial_connection_state: Option<String>,
    /// Power factor magnitude for the charger/inverter front-end.
    /// Defaults to 1.0 (unity — EV onboard chargers have PFC front-ends at
    /// ~0.99+; default runs bit-identical to pre-reactive behaviour).
    pub power_factor: Option<f64>,
    /// Charger/inverter AC apparent-power rating (VA). Used as the VA
    /// clamp for reactive power (active-power priority: P never curtailed).
    /// Defaults to `max(max_charging_power_w, v2g_max_discharge_w,
    /// v2l_max_discharge_w)` at init time.
    pub charger_capacity_va: Option<f64>,
    /// SOC above which CC-CV tapering begins (e.g. 0.85 for NMC).
    /// Below this SOC, charging power is constant; above it, power
    /// tapers linearly toward `CC_CV_MIN_MULTIPLIER` at SOC=1.0.
    /// Ignored when a charging-curve LUT is present — the LUT already
    /// captures the power roll-off.
    pub cc_cv_transition_soc: Option<f64>,
    /// How the EV resolves conflicts between an external PowerSetpoint and
    /// the internal Ready‑By departure deadline. Defaults to
    /// [`ChargingPriority::DeadlineGuarantee`], matching real‑world smart
    /// EVSE behaviour where cost optimization yields to departure readiness.
    /// Set to `ExternalAuthority` for VPP / grid‑service deployments where
    /// the external controller bears sole responsibility for meeting the
    /// departure SOC.
    #[serde(default)]
    pub charging_priority: Option<ChargingPriority>,
    /// When `true` (default), V2L/V2G discharge is stopped at
    /// `max(reserve, ready_by_soc)` when a Ready‑By departure deadline is
    /// active, preventing discharge from compromising departure readiness.
    /// Set to `false` to allow intentional discharge below the deadline
    /// target (e.g. emergency backup power where departure compromise is
    /// acceptable).
    #[serde(default = "default_true")]
    pub discharge_respects_deadline: bool,
}

impl EquipmentTypedConfig for EvConfig {
    fn equipment_type_name() -> &'static str {
        "EV"
    }
}

fn default_true() -> bool {
    true
}

impl EvConfig {
    /// Validate all fields for physical plausibility.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.capacity_j.is_finite() || self.capacity_j <= 0.0 {
            return Err(HaresError::Equipment(
                "EV capacity_j must be finite and > 0".to_string(),
            ));
        }
        if !self.max_charging_power_w.is_finite() || self.max_charging_power_w <= 0.0 {
            return Err(HaresError::Equipment(
                "EV max_charging_power_w must be finite and > 0".to_string(),
            ));
        }
        if let Some(eff) = self.charging_efficiency
            && (eff <= 0.0 || eff > 1.0 || !eff.is_finite())
        {
            return Err(HaresError::Equipment(
                "EV charging_efficiency must be finite and within (0, 1]".to_string(),
            ));
        }
        if let Some(soc_max) = self.soc_max
            && (!soc_max.is_finite() || !(0.0..=1.0).contains(&soc_max))
        {
            return Err(HaresError::Equipment(
                "EV soc_max must be finite and within [0, 1]".to_string(),
            ));
        }
        if let Some(soc) = self.initial_soc
            && (!soc.is_finite() || !(0.0..=1.0).contains(&soc))
        {
            return Err(HaresError::Equipment(
                "EV initial_soc must be finite and within [0, 1]".to_string(),
            ));
        }
        if let Some(current) = self.l1_current_a
            && (!current.is_finite() || current <= 0.0)
        {
            return Err(HaresError::Equipment(
                "EV l1_current_a must be finite and > 0".to_string(),
            ));
        }
        if let Some(voltage) = self.l1_voltage_v
            && (!voltage.is_finite() || voltage <= 0.0)
        {
            return Err(HaresError::Equipment(
                "EV l1_voltage_v must be finite and > 0".to_string(),
            ));
        }
        if let Some(power_w) = self.heater_power_w
            && (!power_w.is_finite() || power_w < 0.0)
        {
            return Err(HaresError::Equipment(
                "EV heater_power_w must be finite and >= 0".to_string(),
            ));
        }
        // Temperature fields must be finite: a NaN temperature falls
        // through both comparison branches of `linear_temp_derate`
        // (lib.rs) into the interpolation and yields a NaN charge derate,
        // silently poisoning charging power, SOC, and every downstream
        // energy total with no error anywhere. Physically extreme but
        // finite temperatures are legal — the plausibility envelope is the
        // degradation guard's job at runtime, not the config boundary's.
        for (name, val) in [
            ("battery_temp_c", self.battery_temp_c),
            ("min_charge_temp_c", self.min_charge_temp_c),
            ("full_power_temp_c", self.full_power_temp_c),
            ("heater_threshold_c", self.heater_threshold_c),
        ] {
            if let Some(t) = val
                && !t.is_finite()
            {
                return Err(HaresError::Equipment(format!(
                    "EV {name} must be finite, got {t}"
                )));
            }
        }
        // The derate ramp must be ordered: with an inverted
        // min_charge_temp_c > full_power_temp_c pair,
        // `linear_temp_derate`'s `temp >= temp_max` branch wins everywhere
        // above temp_max, silently charging at full power across the band
        // the configuration meant to derate. An equal pair is legal — a
        // step cutoff at the plating boundary, not a ramp.
        if let (Some(min_c), Some(full_c)) = (self.min_charge_temp_c, self.full_power_temp_c)
            && min_c > full_c
        {
            return Err(HaresError::Equipment(format!(
                "EV min_charge_temp_c ({min_c} °C) must not exceed \
                 full_power_temp_c ({full_c} °C) — the derate ramp would be \
                 inverted and charge at full power across the derate band"
            )));
        }
        if let Some(mass) = self.thermal_mass_j_per_k
            && (!mass.is_finite() || mass <= 0.0)
        {
            return Err(HaresError::Equipment(
                "EV thermal_mass_j_per_k must be finite and > 0".to_string(),
            ));
        }
        if let Some(ua) = self.ua_w_per_k
            && (!ua.is_finite() || ua < 0.0)
        {
            return Err(HaresError::Equipment(
                "EV ua_w_per_k must be finite and >= 0".to_string(),
            ));
        }
        if let Some(n) = self.n_series
            && n == 0
        {
            return Err(HaresError::Equipment(
                "EV n_series must be >= 1".to_string(),
            ));
        }
        if let Some(n) = self.n_parallel
            && n == 0
        {
            return Err(HaresError::Equipment(
                "EV n_parallel must be >= 1".to_string(),
            ));
        }
        if let Some(r) = self.cell_resistance_ohm
            && (!r.is_finite() || r <= 0.0)
        {
            return Err(HaresError::Equipment(
                "EV cell_resistance_ohm must be finite and > 0".to_string(),
            ));
        }
        for (name, val) in [
            ("v2l_soc_reserve", self.v2l_soc_reserve),
            ("v2g_soc_reserve", self.v2g_soc_reserve),
            ("ready_soc", self.ready_soc),
        ] {
            if let Some(v) = val
                && (!v.is_finite() || !(0.0..=1.0).contains(&v))
            {
                return Err(HaresError::Equipment(format!(
                    "EV {name} must be finite and within [0, 1]"
                )));
            }
        }
        for (name, val) in [
            ("v2l_max_discharge_w", self.v2l_max_discharge_w),
            ("v2g_max_discharge_w", self.v2g_max_discharge_w),
        ] {
            if let Some(v) = val
                && (!v.is_finite() || v < 0.0)
            {
                return Err(HaresError::Equipment(format!(
                    "EV {name} must be finite and >= 0"
                )));
            }
        }
        if let Some(pf) = self.power_factor
            && (!pf.is_finite() || pf <= 0.0 || pf > 1.0)
        {
            return Err(HaresError::Equipment(
                "EV power_factor must be finite and within (0, 1]".to_string(),
            ));
        }
        if let Some(va) = self.charger_capacity_va
            && (!va.is_finite() || va <= 0.0)
        {
            return Err(HaresError::Equipment(
                "EV charger_capacity_va must be finite and > 0".to_string(),
            ));
        }
        if let Some(soc) = self.cc_cv_transition_soc
            && (!soc.is_finite() || !(0.0..=1.0).contains(&soc))
        {
            return Err(HaresError::Equipment(
                "EV cc_cv_transition_soc must be finite and within [0, 1]".to_string(),
            ));
        }
        Ok(())
    }
}

/// The the EV's config's fields as the input schema publishes them.
impl hares_types::DescribeFields for EvConfig {
    fn fields() -> Vec<hares_types::FieldSchema> {
        crate::describe_fields::field_schemas::<Self>(&[
            (
                "equipment_id",
                hares_types::FieldMeta {
                    description: Some("the explicit equipment id the assembly assigns"),
                    ..Default::default()
                },
            ),
            (
                "capacity_j",
                hares_types::FieldMeta {
                    description: Some("the pack's capacity"),
                    unit: Some("J"),
                    ..Default::default()
                },
            ),
            (
                "charging_level",
                hares_types::FieldMeta {
                    description: Some("the charging level"),
                    ..Default::default()
                },
            ),
            (
                "max_charging_power_w",
                hares_types::FieldMeta {
                    description: Some("the maximum charging power"),
                    unit: Some("W"),
                    ..Default::default()
                },
            ),
            (
                "charging_efficiency",
                hares_types::FieldMeta {
                    description: Some("the charging path's efficiency"),
                    ..Default::default()
                },
            ),
            (
                "l1_current_a",
                hares_types::FieldMeta {
                    description: Some("the level-1 charger's current, amperes"),
                    ..Default::default()
                },
            ),
            (
                "l1_voltage_v",
                hares_types::FieldMeta {
                    description: Some("the level-1 charger's voltage, volts"),
                    ..Default::default()
                },
            ),
            (
                "soc_max",
                hares_types::FieldMeta {
                    description: Some("the maximum state of charge"),
                    ..Default::default()
                },
            ),
            (
                "initial_soc",
                hares_types::FieldMeta {
                    description: Some("the initial state of charge"),
                    ..Default::default()
                },
            ),
            (
                "battery_temp_c",
                hares_types::FieldMeta {
                    description: Some("the battery's initial temperature, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "min_charge_temp_c",
                hares_types::FieldMeta {
                    description: Some("the minimum charge temperature, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "full_power_temp_c",
                hares_types::FieldMeta {
                    description: Some("the temperature above which full power flows, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "heater_power_w",
                hares_types::FieldMeta {
                    description: Some("the battery heater's power, watts"),
                    ..Default::default()
                },
            ),
            (
                "heater_threshold_c",
                hares_types::FieldMeta {
                    description: Some("the battery heater's threshold, Celsius"),
                    ..Default::default()
                },
            ),
            (
                "thermal_mass_j_per_k",
                hares_types::FieldMeta {
                    description: Some("the battery's thermal mass, J/K"),
                    ..Default::default()
                },
            ),
            (
                "ua_w_per_k",
                hares_types::FieldMeta {
                    description: Some("the battery's UA, W/K"),
                    ..Default::default()
                },
            ),
            (
                "n_series",
                hares_types::FieldMeta {
                    description: Some("the series cell count"),
                    ..Default::default()
                },
            ),
            (
                "n_parallel",
                hares_types::FieldMeta {
                    description: Some("the parallel cell count"),
                    ..Default::default()
                },
            ),
            (
                "cell_resistance_ohm",
                hares_types::FieldMeta {
                    description: Some("the cell's internal resistance, ohms"),
                    ..Default::default()
                },
            ),
            (
                "v2l_enabled",
                hares_types::FieldMeta {
                    description: Some("whether vehicle-to-load is enabled"),
                    ..Default::default()
                },
            ),
            (
                "v2l_soc_reserve",
                hares_types::FieldMeta {
                    description: Some("the vehicle-to-load's SOC reserve"),
                    ..Default::default()
                },
            ),
            (
                "v2l_max_discharge_w",
                hares_types::FieldMeta {
                    description: Some("the vehicle-to-load's maximum discharge"),
                    unit: Some("W"),
                    ..Default::default()
                },
            ),
            (
                "v2g_enabled",
                hares_types::FieldMeta {
                    description: Some("whether vehicle-to-grid is enabled"),
                    ..Default::default()
                },
            ),
            (
                "v2g_soc_reserve",
                hares_types::FieldMeta {
                    description: Some("the vehicle-to-grid's SOC reserve"),
                    ..Default::default()
                },
            ),
            (
                "v2g_max_discharge_w",
                hares_types::FieldMeta {
                    description: Some("the vehicle-to-grid's maximum discharge"),
                    unit: Some("W"),
                    ..Default::default()
                },
            ),
            (
                "chemistry",
                hares_types::FieldMeta {
                    description: Some("the cell chemistry"),
                    ..Default::default()
                },
            ),
            (
                "fuel_economy_j_per_m",
                hares_types::FieldMeta {
                    description: Some("the vehicle's consumption, J/m"),
                    ..Default::default()
                },
            ),
            (
                "ready_soc",
                hares_types::FieldMeta {
                    description: Some("the departure-ready SOC"),
                    ..Default::default()
                },
            ),
            (
                "charging_strategy",
                hares_types::FieldMeta {
                    description: Some("the charging strategy"),
                    ..Default::default()
                },
            ),
            (
                "plug_in_policy",
                hares_types::FieldMeta {
                    description: Some("the plug-in policy"),
                    ..Default::default()
                },
            ),
            (
                "power_limit_w",
                hares_types::FieldMeta {
                    description: Some("the charging power limit"),
                    unit: Some("W"),
                    ..Default::default()
                },
            ),
            (
                "initial_connection_state",
                hares_types::FieldMeta {
                    description: Some("the initial connection state"),
                    ..Default::default()
                },
            ),
            (
                "power_factor",
                hares_types::FieldMeta {
                    description: Some("the power factor"),
                    ..Default::default()
                },
            ),
            (
                "charger_capacity_va",
                hares_types::FieldMeta {
                    description: Some("the charger's apparent-power capacity"),
                    unit: Some("VA"),
                    ..Default::default()
                },
            ),
            (
                "cc_cv_transition_soc",
                hares_types::FieldMeta {
                    description: Some("the CC-CV transition's SOC"),
                    ..Default::default()
                },
            ),
            (
                "charging_priority",
                hares_types::FieldMeta {
                    description: Some("the charging priority"),
                    ..Default::default()
                },
            ),
            (
                "discharge_respects_deadline",
                hares_types::FieldMeta {
                    description: Some("whether discharge respects the deadline"),
                    ..Default::default()
                },
            ),
        ])
    }
}
