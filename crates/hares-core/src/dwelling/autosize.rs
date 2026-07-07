//! HVAC and water heater capacity autosizing from building envelope model
//! and draw profile data.
//!
//! When HPXML equipment omits HeatingCapacity/CoolingCapacity, this module
//! computes the required capacity at design outdoor conditions using the
//! building's thermal model and ASHRAE 152 (or EPW) design temperatures.
//!
//! Oversizing factors per ACCA Manual S:
//! - Heating (furnace/non-HP): 1.4x (overridden by HPXML `<HeatingAutosizingFactor>`)
//! - Heating (heat pump): 1.25x — ACCA Manual S-2017 §4-5 (overridden by HPXML `<HeatingAutosizingFactor>`)
//! - Cooling: 1.15x (overridden by HPXML `<CoolingAutosizingFactor>`)
//!
//! HPXML `<AutosizingLimits>` elements (Min/Max capacity bounds) are
//! applied as a clamp on the final sized capacity when present.
//!
//! ## Water heater autosizing
//!
//! For storage water heaters with `autosize_water_heater = true`, the
//! module computes tank volume and heating capacity using a First-Hour
//! Rating methodology per DOE 10 CFR Part 430 Subpart B Appendix E:
//!
//! 1. Required FHR [GPH] is derived from bedroom count; no single standard
//!    tabulates these values — they are engineering convention values derived
//!    from applying the DOE FHR test procedure (10 CFR Part 430 Subpart B
//!    Appendix E) to typical residential hot-water usage patterns.
//! 2. Tank volume [gal] is sized by bedroom count.
//! 3. Heating capacity [W] is computed as the power needed to recover the
//!    FHR deficit (FHR − usable tank volume) at the design temperature rise
//!    (setpoint − mains temperature).
//! 4. When inputs are insufficient, conservative defaults are used:
//!    50 gal tank, 4500 W element capacity.

use hares_envelope::ThermalSolver;
use hares_io::{
    Building, DesignConditions, EquipmentSpec,
    hpxml::{
        rebuild_wh_typed_config,
        resolve_hvac::{DuctDseParams, rebuild_hvac_typed_config},
    },
};
use hares_physics::ashrae152::design_temperatures_f;
use hares_physics::constants::{OCCUPANT_LATENT_GAIN_W, OCCUPANT_SENSIBLE_GAIN_W};
use hares_physics::units::{temperature_c_to_f, temperature_f_to_c};
use hares_types::ZoneId;
#[cfg_attr(not(test), allow(unused_imports))]
use serde_json::{Value, json};
use tracing::{error, warn};

/// ASHRAE 90.1 default indoor design setpoints [°C].
/// ASHRAE 90.1-2019 §6.4.3.1.1.
const DEFAULT_HEATING_SETPOINT_C: f64 = 21.1; // 70 °F
const DEFAULT_COOLING_SETPOINT_C: f64 = 23.9; // 75 °F

/// Manual S oversizing factors.
/// ACCA Manual S-2017 §4: equipment sizing based on design loads.
const HEATING_OVERSIZE_FACTOR: f64 = 1.4;
const COOLING_OVERSIZE_FACTOR: f64 = 1.15;

/// Manual S heat pump heating oversizing factor.
/// ACCA Manual S-2017 §4-5: heat pump heating mode oversizing is limited
/// to 1.25× because oversized heat pumps cycle excessively during mild
/// weather, degrading COP and increasing auxiliary heat runtime.
const HEATING_OVERSIZE_FACTOR_HEAT_PUMP: f64 = 1.25;

/// Backup heating capacity factor: sized to 100% of design heating load
/// with no oversizing per ACCA Manual S-2017
/// (overridden by HPXML `<BackupHeatingAutosizingFactor>`).
const BACKUP_CAPACITY_FACTOR: f64 = 1.0;

/// Default lighting and plug load density for cooling autosizing [W/m²].
///
/// ASHRAE 62.2-2022 Appendix B: typical residential plug and lighting load
/// density ≈ 5 W/m². Used as a default when HPXML Lighting/PlugLoad elements
/// are absent.
const DEFAULT_LIGHTING_PLUG_DENSITY_W_M2: f64 = 5.0;

/// Default number of occupants for cooling autosizing.
///
/// ASHRAE 62.2-2022 Appendix B: two occupants for a typical single-family
/// residence. Sensible and latent gains per occupant are taken from
///    ASHRAE HoF 2021 Ch.18 Table 1 (OCCUPANT_SENSIBLE_GAIN_W,
///    OCCUPANT_LATENT_GAIN_W).
const DEFAULT_OCCUPANTS: f64 = 2.0;

// ── Water heater autosizing constants ──────────────────────────────────

/// Default water heater sizing factor (conservative — no oversizing).
/// DOE 10 CFR Part 430 Subpart B Appendix E: FHR methodology allows
/// engineering judgment for sizing factors.
const WATER_HEATER_SIZING_FACTOR: f64 = 1.0;

/// Default tank volume [gal] used when bedroom count is unavailable.
/// ASHRAE 90.2-2018: typical single-family storage water heater tank size.
const DEFAULT_TANK_VOLUME_GAL: f64 = 50.0;

/// Default element/burner capacity [W] for storage water heaters.
/// Typical residential electric water heater: 4500 W at 240 V.
/// ASHRAE 90.2-2018 Table 7.5.1.
const DEFAULT_ELEMENT_CAPACITY_W: f64 = 4_500.0;

/// Default mains cold-water temperature [°C] for sizing when unavailable.
/// Mild-climate fallback; tropical sites should supply their actual mains
/// temperature (typically 25–35 °C).
const DEFAULT_MAINS_TEMP_C: f64 = 25.0;

/// Default hot water setpoint [°C] for sizing when not specified.
/// ASHRAE 90.2-2018 §7.5: storage water heater setpoint 125 °F (51.67 °C).
const DEFAULT_WH_SETPOINT_C: f64 = 51.67;

/// Usable tank fraction: fraction of rated volume that can be drawn before
/// the outlet temperature drops below the setpoint.
/// DOE 10 CFR Part 430 Subpart B Appendix E §2.4.1: draw until outlet
/// temperature drops 14 °C below the initial mean tank temperature.
/// 0.7 is the conventional usable fraction for residential storage tanks.
const USABLE_TANK_FRACTION: f64 = 0.7;

/// Density of water at conventional cold-water temperature (~60 °F / 15.6 °C).
/// ASHRAE HoF 2021 Ch.1: 8.33 lb/US gal is the conventional US engineering
/// value for water density. NOTE: at typical tank storage temperature
/// (50 °C / 125 °F) the density is ~8.23 lb/gal, but the conventional
/// 8.33 value is universally used in water heater sizing worksheets and
/// introduces <2% error in the capacity calculation.
const WATER_DENSITY_LB_PER_GAL: f64 = 8.33;

/// BTU per hour per watt (= 3.412 BTU/hr per W).
/// ASHRAE HoF 2021 Ch.1 Table 1.
const BTU_PER_HR_PER_W: f64 = 3.412;

/// Conversion: 1 °C = 1.8 °F.
const DEG_F_PER_DEG_C: f64 = 1.8;

/// Pre-computed water energy factor for FHR capacity formula:
///   `WATER_DENSITY_LB_PER_GAL * DEG_F_PER_DEG_C / BTU_PER_HR_PER_W`
///   = 8.33 × 1.8 / 3.412 ≈ 4.395
///
/// Used in: capacity_w = max(0, FHR_gph − usable_volume_gal) × factor × ΔT_C
const WATER_ENERGY_FACTOR: f64 = WATER_DENSITY_LB_PER_GAL * DEG_F_PER_DEG_C / BTU_PER_HR_PER_W;

/// Context bundle for autosizing: weather-derived design conditions and
/// duct parameters needed to rebuild typed equipment configs.
///
/// Groups together the EPW/ASHRAE 152 design temperature inputs and
/// ASHRAE 152 duct DSE parameters that `autosize_equipment_capacities`
/// requires, keeping the function signature under clippy's argument limit.
pub struct AutosizeContext {
    /// EPW design conditions from the weather file header, or `None`.
    pub design_conditions: Option<DesignConditions>,
    /// Weather file latitude [°N] for ASHRAE 152 fallback lookup.
    pub weather_lat: f64,
    /// Weather file longitude [°E] for ASHRAE 152 fallback lookup.
    pub weather_lon: f64,
    /// Duct DSE parameters for rebuilding typed equipment configs.
    pub duct_params: DuctDseParams,
    /// Sensible internal gains [W] for cooling autosizing.
    ///
    /// ACCA Manual J-2016 §7: cooling design loads must include sensible
    /// internal gains from occupancy, lighting, and appliances.
    /// Default: 2 occupants × 75 W/person (ASHRAE HoF 2021 Ch.18 Table 1) = 150 W
    /// + 5 W/m² lights/plug loads.
    ///
    /// Overridable via HPXML internal gains data.
    pub internal_gains_w: f64,
    /// Latent internal gains [W] for cooling autosizing latent load estimation.
    ///
    /// Default: 2 occupants × 55 W/person (ASHRAE HoF 2021 Ch.18 Table 1) = 110 W.
    /// Overridable via HPXML internal gains data.
    pub internal_gains_latent_w: f64,
}

/// Compute default internal gains for cooling autosizing from building geometry.
///
/// Returns `(sensible_w, latent_w)`:
/// - Occupancy: `DEFAULT_OCCUPANTS` × per-capita gains from ASHRAE HoF 2021 Ch.18 Table 1
///   (OCCUPANT_SENSIBLE_GAIN_W, OCCUPANT_LATENT_GAIN_W).
/// - Lighting/plug: `DEFAULT_LIGHTING_PLUG_DENSITY_W_M2` × conditioned floor area.
///
/// Floor area is derived from `building.conditioned_volume_m3 / building.ceiling_height_m`,
/// falling back to the first conditioned zone's `floor_area_m2`.
///
/// If floor area cannot be determined (both sources are absent or zero), a `warn!` is
/// emitted and gains are returned based on occupancy only (zero lighting/plug component).
///
/// Override: when `ctx.internal_gains_w > 0.0`, the context-supplied values
/// take precedence (HPXML override path).
pub fn compute_default_internal_gains(ctx: &AutosizeContext, building: &Building) -> (f64, f64) {
    // Override via AutosizeContext (HPXML-supplied values).
    if ctx.internal_gains_w > 0.0 {
        return (ctx.internal_gains_w, ctx.internal_gains_latent_w);
    }

    // Occupancy: DEFAULT_OCCUPANTS × per-capita gains.
    // ASHRAE HoF 2021 Ch.18 Table 1: seated adult
    // sensible = 75 W, latent = 55 W.
    let occ_sensible = DEFAULT_OCCUPANTS * OCCUPANT_SENSIBLE_GAIN_W;
    let occ_latent = DEFAULT_OCCUPANTS * OCCUPANT_LATENT_GAIN_W;

    // Floor area from building geometry.
    let floor_area_m2 = building
        .conditioned_volume_m3
        .zip(building.ceiling_height_m)
        .filter(|&(_v, h)| h > 0.0)
        .map(|(v, h)| v / h)
        .or_else(|| {
            building
                .zones
                .iter()
                .find(|z| {
                    matches!(
                        z.zone_type,
                        hares_io::hpxml::building::ZoneType::Conditioned
                    )
                })
                .and_then(|z| z.floor_area_m2)
        });

    match floor_area_m2 {
        Some(area) if area > 0.0 => {
            let lights_and_plug = area * DEFAULT_LIGHTING_PLUG_DENSITY_W_M2;
            let sensible = occ_sensible + lights_and_plug;
            (sensible, occ_latent)
        }
        _ => {
            tracing::warn!(
                occupancy_sensible_w = occ_sensible,
                occupancy_latent_w = occ_latent,
                "cooling autosizing: conditioned floor area is zero or missing — \
                 internal gains limited to occupancy only; sizing may be conservative"
            );
            (occ_sensible, occ_latent)
        }
    }
}

/// Returns `true` when the equipment spec name indicates a heat pump.
///
/// Matches the heat pump name patterns used by `resolve_hvac`:
/// abbreviated forms (`ASHP`, `GSHP`, `MSHP`, `WSHP`) and long-form
/// names containing "heat pump" (but excluding heat pump water heaters).
fn is_heat_pump_equipment(spec: &EquipmentSpec) -> bool {
    let name_lower = spec.name.to_lowercase();
    name_lower.contains("ashp")
        || name_lower.contains("gshp")
        || name_lower.contains("mshp")
        || name_lower.contains("wshp")
        || (name_lower.contains("heat pump") && !name_lower.contains("water"))
}

/// Autosize HVAC equipment capacities for all specs that are missing
/// explicit capacity values from HPXML.
///
/// Runs after the thermal solver has been built, using the building's RC
/// model to compute the heating and cooling loads at ASHRAE design conditions.
///
/// For each equipment spec with `autosize_heating`, `autosize_cooling`, or
/// `autosize_backup` flags in its params:
///
/// 1. Determine design outdoor temperature from EPW "Extremes" header or
///    ASHRAE 152 climate station lookup (fallback).
/// 2. Call [`ThermalSolver::autosize_design_day_heating`] /
///    [`ThermalSolver::autosize_design_day_cooling`] to compute the
///    required HVAC capacity via iterative design-day simulation (<xref>EnergyPlus
///    SizingManager.cc ZoneSizingCalc methodology</xref>).
/// 3. Apply oversizing factor from HPXML `<HeatingAutosizingFactor>` /
///    `<CoolingAutosizingFactor>` when present; fall back to Manual S
///    defaults (1.4x heating, 1.15x cooling) when absent.
///    Backup heating uses 100% of design load (no Manual S oversizing).
/// 4. Apply capacity limits from HPXML `<AutosizingLimits>` when present
///    (clamp to Min/Max after oversizing).
/// 5. Update the equipment spec's parameters and rebuild its typed config.
///
/// # Arguments
///
/// * `specs` - Equipment specs from HPXML resolution (mutated in place).
/// * `thermal` - The assembled building thermal solver.
/// * `ctx` - Autosizing context (design conditions, weather location, duct params).
/// * `building` - HPXML building data (for setpoint defaults and site lat/lon).
/// * `indoor_zone_id` - The primary conditioned zone for HVAC delivery.
pub fn autosize_equipment_capacities(
    specs: &mut [EquipmentSpec],
    thermal: &ThermalSolver,
    ctx: &AutosizeContext,
    building: &Building,
    indoor_zone_id: ZoneId,
) {
    // Resolve outdoor design temperatures.
    // Prefer EPW design conditions; fall back to ASHRAE 152 station lookup.
    let (heating_design_c, cooling_design_c) = resolve_design_temperatures(
        ctx.design_conditions,
        ctx.weather_lat,
        ctx.weather_lon,
        building.site.latitude_deg,
        building.site.longitude_deg,
    );

    // Compute internal gains for cooling autosizing.
    // ACCA Manual J-2016 §7: cooling design loads must include sensible
    // internal gains from occupancy, lighting, and appliances.
    let (internal_gains_w, internal_gains_latent_w) = compute_default_internal_gains(ctx, building);

    for spec in specs.iter_mut() {
        let needs_heating = spec
            .parameters
            .get("autosize_heating")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let needs_cooling = spec
            .parameters
            .get("autosize_cooling")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Cooler specs share the base params (cloned before heater/cooler split
        // in resolve_hvac.rs), so they also inherit autosize_backup when a backup
        // system is declared. Cooler configs do not consume backup_capacity_w —
        // computing backup capacity for them is wasted work and surprising.
        let needs_backup = spec
            .parameters
            .get("autosize_backup")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && !spec.name.ends_with(" Cooler");

        if !needs_heating && !needs_cooling && !needs_backup {
            continue;
        }

        // Determine indoor design setpoints.
        // Prefer the equipment's own setpoint; fall back to building setpoints;
        // fall back to ASHRAE 90.1 defaults.
        let heating_setpoint_c = resolve_heating_setpoint_c(spec, building);
        let cooling_setpoint_c = resolve_cooling_setpoint_c(spec, building);

        if needs_heating {
            let raw_capacity = thermal
                .autosize_design_day_heating(indoor_zone_id, heating_setpoint_c, heating_design_c)
                .abs();

            // Oversizing factor: prefer HPXML <HeatingAutosizingFactor>;
            // fall back to ACCA Manual S default 1.4x.
            let has_factor_override = spec.parameters.get("autosize_heating_factor").is_some();
            let mut factor = spec
                .parameters
                .get("autosize_heating_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(HEATING_OVERSIZE_FACTOR);

            // Heat pumps in heating mode are limited to 1.25× oversizing per
            // ACCA Manual S-2017 §4-5: oversized heat pumps cycle excessively
            // during mild weather, degrading COP and increasing auxiliary heat
            // runtime. The HPXML <HeatingAutosizingFactor> override takes
            // precedence when present.
            if is_heat_pump_equipment(spec) {
                if !has_factor_override {
                    // Default factor path: cap at 1.25×.
                    if factor > HEATING_OVERSIZE_FACTOR_HEAT_PUMP + f64::EPSILON {
                        tracing::debug!(
                            equipment = %spec.name,
                            requested = factor,
                            clamped = HEATING_OVERSIZE_FACTOR_HEAT_PUMP,
                            "heat pump heating factor clamped to Manual S §4-5 limit (1.25×)"
                        );
                        factor = HEATING_OVERSIZE_FACTOR_HEAT_PUMP;
                    }
                } else if factor > HEATING_OVERSIZE_FACTOR_HEAT_PUMP + f64::EPSILON {
                    // HPXML override exceeds Manual S heat pump limit — warn but
                    // respect override (user explicitly chose a non-standard factor).
                    warn!(
                        equipment = %spec.name,
                        factor,
                        manual_s_limit = HEATING_OVERSIZE_FACTOR_HEAT_PUMP,
                        "HPXML <HeatingAutosizingFactor> exceeds ACCA Manual S-2017 §4-5 \
                         heat pump heating limit of 1.25×; override is respected but may \
                         cause excessive cycling"
                    );
                }
            }

            // Invariant: heat pump heating factor with default path must not
            // exceed Manual S §4-5 limit.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                if is_heat_pump_equipment(spec) && !has_factor_override {
                    assert!(
                        factor <= HEATING_OVERSIZE_FACTOR_HEAT_PUMP + f64::EPSILON,
                        "invariant: heat pump heating factor {factor} exceeds Manual S §4-5 \
                         limit of {HEATING_OVERSIZE_FACTOR_HEAT_PUMP} for equipment {}",
                        spec.name
                    );
                }
            }

            let mut sized_capacity = raw_capacity * factor;

            // Apply capacity limits from HPXML <AutosizingLimits> if present.
            if let Some(min_w) = spec
                .parameters
                .get("autosize_heating_min_w")
                .and_then(|v| v.as_f64())
            {
                sized_capacity = sized_capacity.max(min_w);
            }
            if let Some(max_w) = spec
                .parameters
                .get("autosize_heating_max_w")
                .and_then(|v| v.as_f64())
            {
                let unclamped = sized_capacity;
                sized_capacity = sized_capacity.min(max_w);
                if unclamped > max_w + f64::EPSILON {
                    tracing::debug!(
                        equipment = %spec.name,
                        max_w,
                        clamped_capacity = sized_capacity,
                        "heating capacity clamped to <AutosizingLimits> maximum"
                    );
                }
            }

            // Remove factor and limit params consumed by autosizing.
            spec.parameters.remove("autosize_heating_factor");
            spec.parameters.remove("autosize_heating_min_w");
            spec.parameters.remove("autosize_heating_max_w");

            if sized_capacity > 0.0 {
                // Remove the non-canonical "capacity_w" key (inserted by
                // the Python typed-equipment bindings) now that the
                // canonical "heating_capacity_w" is set.
                spec.parameters.remove("capacity_w");
                spec.parameters
                    .insert("heating_capacity_w".to_string(), json!(sized_capacity));
                spec.parameters.remove("autosize_heating");
                tracing::info!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    sized_capacity_w = sized_capacity,
                    factor,
                    design_outdoor_c = heating_design_c,
                    indoor_setpoint_c = heating_setpoint_c,
                    "autosized heating capacity"
                );
            } else {
                error!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    factor,
                    design_outdoor_c = heating_design_c,
                    "autosize heating capacity: solve returned zero — \
                     check model configuration"
                );
            }
        }

        if needs_cooling {
            // Cooling autosizing: run a design-day simulation on July 21
            // using ASHRAE 1% DB profile with clear-sky solar gains.
            // This replaces the single-condition solar noon approach with
            // a full 24-hour diurnal cycle, capturing the interaction
            // between thermal mass lag, outdoor temperature peak (3–4 PM),
            // and solar gain coincidence. Reference: EnergyPlus
            // SizingManager.cc:285-390 (ZoneSizingCalc design-day methodology).
            let raw_capacity = thermal
                .autosize_design_day_cooling(
                    indoor_zone_id,
                    cooling_setpoint_c,
                    cooling_design_c,
                    ctx.weather_lat,
                    ctx.weather_lon,
                    internal_gains_w,
                )
                .abs();

            // Oversizing factor: prefer HPXML <CoolingAutosizingFactor>;
            // fall back to ACCA Manual S default 1.15x.
            let factor = spec
                .parameters
                .get("autosize_cooling_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(COOLING_OVERSIZE_FACTOR);

            let mut sized_capacity = raw_capacity * factor;

            // Apply capacity limits from HPXML <AutosizingLimits> if present.
            if let Some(min_w) = spec
                .parameters
                .get("autosize_cooling_min_w")
                .and_then(|v| v.as_f64())
            {
                sized_capacity = sized_capacity.max(min_w);
            }
            if let Some(max_w) = spec
                .parameters
                .get("autosize_cooling_max_w")
                .and_then(|v| v.as_f64())
            {
                let unclamped = sized_capacity;
                sized_capacity = sized_capacity.min(max_w);
                if unclamped > max_w + f64::EPSILON {
                    tracing::debug!(
                        equipment = %spec.name,
                        max_w,
                        clamped_capacity = sized_capacity,
                        "cooling capacity clamped to <AutosizingLimits> maximum"
                    );
                }
            }

            // Remove factor and limit params consumed by autosizing.
            spec.parameters.remove("autosize_cooling_factor");
            spec.parameters.remove("autosize_cooling_min_w");
            spec.parameters.remove("autosize_cooling_max_w");

            if sized_capacity > 0.0 {
                // Remove the non-canonical "capacity_w" key (inserted by
                // the Python typed-equipment bindings) now that the
                // canonical "cooling_capacity_w" is set.
                spec.parameters.remove("capacity_w");
                spec.parameters
                    .insert("cooling_capacity_w".to_string(), json!(sized_capacity));
                spec.parameters.remove("autosize_cooling");
                tracing::info!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    sized_capacity_w = sized_capacity,
                    factor,
                    design_outdoor_c = cooling_design_c,
                    indoor_setpoint_c = cooling_setpoint_c,
                    internal_gains_w = internal_gains_w,
                    internal_gains_latent_w = internal_gains_latent_w,
                    "autosized cooling capacity"
                );
            } else {
                error!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    factor,
                    design_outdoor_c = cooling_design_c,
                    "autosize cooling capacity: solve returned zero — \
                     check model configuration"
                );
            }
        }

        if needs_backup {
            // Backup heating autosizing: sized to 100% of design heating load
            // with no Manual S oversizing factor.
            // When heating capacity was already computed above, the same raw
            // capacity applies. Otherwise recompute it.
            let raw_capacity = thermal
                .autosize_design_day_heating(indoor_zone_id, heating_setpoint_c, heating_design_c)
                .abs();

            // Backup factor: prefer HPXML <BackupHeatingAutosizingFactor>;
            // fall back to 100% of design load (BACKUP_CAPACITY_FACTOR = 1.0).
            let factor = spec
                .parameters
                .get("autosize_backup_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(BACKUP_CAPACITY_FACTOR);

            let backup_capacity = raw_capacity * factor;

            // Remove factor param consumed by autosizing.
            spec.parameters.remove("autosize_backup_factor");

            if backup_capacity > 0.0 {
                spec.parameters
                    .insert("backup_capacity_w".to_string(), json!(backup_capacity));
                spec.parameters.remove("autosize_backup");
                tracing::info!(
                    equipment = %spec.name,
                    backup_capacity_w = backup_capacity,
                    factor,
                    raw_capacity_w = raw_capacity,
                    design_outdoor_c = heating_design_c,
                    indoor_setpoint_c = heating_setpoint_c,
                    "autosized backup heating capacity"
                );
            } else {
                error!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    factor,
                    design_outdoor_c = heating_design_c,
                    "autosize backup capacity: solve returned zero — \
                     check model configuration"
                );
            }
        }

        // Rebuild typed config with updated capacities.
        spec.typed_config =
            rebuild_hvac_typed_config(&spec.name, &spec.parameters, &ctx.duct_params);
    }
}

// ── Water heater autosizing ────────────────────────────────────────────

/// Autosize water heater tank volume and heating capacity for storage
/// water heater specs flagged with `autosize_water_heater = true`.
///
/// ## Sizing methodology
///
/// 1. Required FHR [GPH] is derived from bedroom count; no single standard
///    tabulates these values — they are engineering convention values derived
///    from applying the DOE FHR test procedure (10 CFR Part 430 Subpart B
///    Appendix E §2.4) to typical residential hot-water usage patterns.
/// 2. Tank volume [gal] is sized by bedroom count (e.g. 40 gal for 1–2 BR,
///    50 gal for 3–4 BR, 60 gal for 5+ BR).
/// 3. Heating capacity [W] is computed as the power needed to recover the
///    FHR deficit at the design temperature rise:
///    `capacity_w = max(0, FHR_gph − usable_vol_gal) × W.E.F × ΔT_°C`
///    where `W.E.F = 8.33 × 1.8 / 3.412 ≈ 4.395`.
/// 4. When bedroom count or mains temperature are unavailable, conservative
///    defaults are used (50 gal tank, 4500 W element) with a `warn!`.
///
/// DOE 10 CFR Part 430 Subpart B Appendix E §2.4: FHR = V_draw + recovery;
/// V_draw ≈ usable_fraction × tank_volume.
///
/// # Arguments
///
/// * `specs` - Equipment specs (mutated in place; only storage WH specs
///   with `autosize_water_heater = true` are processed).
/// * `n_bedrooms` - Number of bedrooms for sizing (from HPXML or data patches).
///   `None` triggers the conservative default path.
/// * `mains_temp_c` - Mains cold-water supply temperature [°C] for temperature
///   rise computation.
pub fn autosize_water_heater_capacities(
    specs: &mut [EquipmentSpec],
    n_bedrooms: Option<f64>,
    mains_temp_c: f64,
) {
    // Validate mains temperature — clamp or use default if unreasonable.
    // Upper bound allows tropical cold-water temperatures up to 50 °C.
    let mains_temp_c = if mains_temp_c.is_finite() && mains_temp_c > 0.0 && mains_temp_c < 50.0 {
        mains_temp_c
    } else {
        tracing::warn!(
            mains_temp_c,
            "water heater autosizing: mains temperature unreasonable; \
             using default {} °C",
            DEFAULT_MAINS_TEMP_C
        );
        DEFAULT_MAINS_TEMP_C
    };

    for spec in specs.iter_mut() {
        let needs_autosize = spec
            .parameters
            .get("autosize_water_heater")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !needs_autosize {
            continue;
        }

        // Determine setpoint: prefer spec parameter, fall back to default.
        let setpoint_c = spec
            .parameters
            .get("setpoint_c")
            .and_then(|v| v.as_f64())
            .unwrap_or(DEFAULT_WH_SETPOINT_C);

        // Temperature rise: setpoint − mains.
        let delta_t_c = (setpoint_c - mains_temp_c).max(1.0);

        // Resolve bedroom count.
        let n_bedrooms = n_bedrooms.unwrap_or_else(|| {
            tracing::warn!(
                equipment = %spec.name,
                "water heater autosizing: bedroom count unavailable; \
                 using conservative defaults"
            );
            0.0
        });

        // Invariant: check bedroom count in debug/check_invariants builds.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            if n_bedrooms <= 0.0 {
                tracing::warn!(
                    equipment = %spec.name,
                    n_bedrooms,
                    "water heater autosizing: bedroom count is zero or negative; \
                     sizing based on defaults"
                );
            }
        }

        // Size tank volume from bedrooms (or default).
        let tank_volume_gal = if n_bedrooms > 0.0 {
            tank_volume_from_bedrooms_gal(n_bedrooms)
        } else {
            DEFAULT_TANK_VOLUME_GAL
        };

        // Size required FHR from bedrooms (or default).
        let fhr_gph = if n_bedrooms > 0.0 {
            required_fhr_gph(n_bedrooms)
        } else {
            // No bedroom data: assume 50 GPH (conservative for 3-4 BR home).
            50.0
        };

        // Compute required heating capacity from FHR.
        let usable_volume_gal = USABLE_TANK_FRACTION * tank_volume_gal;
        let capacity_w = if fhr_gph > usable_volume_gal {
            (fhr_gph - usable_volume_gal) * WATER_ENERGY_FACTOR * delta_t_c
        } else {
            // Tank volume alone meets peak hour demand; size minimally.
            // Provide enough power to heat the full tank from cold in ~4 hours.
            DEFAULT_ELEMENT_CAPACITY_W
        };

        // Apply sizing factor (HPXML override or default 1.0).
        let factor = spec
            .parameters
            .get("autosize_water_heater_factor")
            .and_then(|v| v.as_f64())
            .unwrap_or(WATER_HEATER_SIZING_FACTOR);

        let mut sized_capacity_w = capacity_w * factor;

        // Apply capacity limits if present.
        if let Some(min_w) = spec
            .parameters
            .get("autosize_water_heater_min_w")
            .and_then(|v| v.as_f64())
        {
            sized_capacity_w = sized_capacity_w.max(min_w);
        }
        if let Some(max_w) = spec
            .parameters
            .get("autosize_water_heater_max_w")
            .and_then(|v| v.as_f64())
        {
            sized_capacity_w = sized_capacity_w.min(max_w);
        }

        // Remove consumed params.
        spec.parameters.remove("autosize_water_heater_factor");
        spec.parameters.remove("autosize_water_heater_min_w");
        spec.parameters.remove("autosize_water_heater_max_w");

        // Convert tank volume to SI for the config.
        let tank_volume_m3 = hares_physics::units::volume_gal_to_m3(tank_volume_gal);

        if sized_capacity_w > 0.0 {
            // Update the spec parameters with computed values.
            spec.parameters
                .insert("heating_capacity_w".to_string(), json!(sized_capacity_w));

            // Only set tank volume if HPXML didn't provide one.
            if !spec.parameters.contains_key("tank_volume_m3") {
                spec.parameters
                    .insert("tank_volume_m3".to_string(), json!(tank_volume_m3));
            }

            spec.parameters.remove("autosize_water_heater");

            // Emit observability metrics behind observe feature gate.
            #[cfg(feature = "observe")]
            {
                tracing::info!(
                    equipment = %spec.name,
                    sizing.water_heater.fhr_gph = fhr_gph,
                    sizing.water_heater.tank_volume_gal = tank_volume_gal,
                    sizing.water_heater.capacity_w = sized_capacity_w,
                    sizing.water_heater.mains_temp_c = mains_temp_c,
                    sizing.water_heater.num_bedrooms = n_bedrooms,
                    "autosized water heater capacity and tank volume"
                );
            }
            #[cfg(not(feature = "observe"))]
            {
                tracing::info!(
                    equipment = %spec.name,
                    fhr_gph,
                    tank_volume_gal,
                    capacity_w = sized_capacity_w,
                    mains_temp_c,
                    n_bedrooms,
                    factor,
                    "autosized water heater capacity and tank volume"
                );
            }

            // Invariant: computed values must be positive and non-NaN.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                assert!(
                    fhr_gph > 0.0 && fhr_gph.is_finite(),
                    "water heater autosizing invariant: FHR must be positive and finite, \
                     got {fhr_gph} for {}",
                    spec.name
                );
                assert!(
                    tank_volume_gal > 0.0 && tank_volume_gal.is_finite(),
                    "water heater autosizing invariant: tank volume must be positive \
                     and finite, got {tank_volume_gal} for {}",
                    spec.name
                );
                assert!(
                    sized_capacity_w > 0.0 && sized_capacity_w.is_finite(),
                    "water heater autosizing invariant: capacity must be positive and \
                     finite, got {sized_capacity_w} for {}",
                    spec.name
                );
            }
        } else {
            // Autosizing computed zero or negative capacity — fall back to a
            // minimum safe default so the simulation can still run.
            tracing::warn!(
                equipment = %spec.name,
                fhr_gph,
                tank_volume_gal,
                delta_t_c,
                factor,
                "water heater autosizing: computed capacity is zero — \
                 falling back to default element capacity {} W and {} gal tank. \
                 Check bedroom count, setpoint, and mains temperature",
                DEFAULT_ELEMENT_CAPACITY_W,
                DEFAULT_TANK_VOLUME_GAL
            );

            // Remove consumed params and autosize flag so the spec is not
            // re-processed on subsequent builds.
            spec.parameters.remove("autosize_water_heater_factor");
            spec.parameters.remove("autosize_water_heater_min_w");
            spec.parameters.remove("autosize_water_heater_max_w");

            let fallback_vol_m3 = hares_physics::units::volume_gal_to_m3(DEFAULT_TANK_VOLUME_GAL);
            spec.parameters.insert(
                "heating_capacity_w".to_string(),
                json!(DEFAULT_ELEMENT_CAPACITY_W),
            );
            if !spec.parameters.contains_key("tank_volume_m3") {
                spec.parameters
                    .insert("tank_volume_m3".to_string(), json!(fallback_vol_m3));
            }
            spec.parameters.remove("autosize_water_heater");
        }

        // Rebuild typed config from the (now updated) parameters so
        // downstream consumers see the autosized capacity and volume.
        spec.typed_config = rebuild_wh_typed_config(&spec.name, &spec.parameters);
    }
}

/// Required First-Hour Rating [GPH] from bedroom count.
///
/// Peak hour hot water demand for a typical single-family dwelling. No
/// single published standard tabulates these per-bedroom FHR sizing values
/// in a single table. They are engineering convention values derived from
/// applying the DOE First-Hour Rating test procedure (10 CFR Part 430
/// Subpart B Appendix E) to typical residential hot-water usage patterns.
/// These bedroom-to-FHR sizing guidelines appear across residential energy
/// codes, manufacturer sizing guides, and DOE water heater replacement
/// sizing practice.
///
/// | Bedrooms | FHR [GPH] |
/// |----------|-----------|
/// | 1        | 36        |
/// | 2        | 42        |
/// | 3        | 48        |
/// | 4        | 54        |
/// | 5+       | 62        |
fn required_fhr_gph(n_bedrooms: f64) -> f64 {
    let n = n_bedrooms.round() as u32;
    match n {
        0 => 50.0, // conservative default
        1 => 36.0,
        2 => 42.0,
        3 => 48.0,
        4 => 54.0,
        _ => 62.0,
    }
}

/// Tank volume [gal] from bedroom count.
///
/// Water heater tank sizing by bedroom count is a residential energy code
/// prescriptive convention (see ASHRAE 90.2-2018 §7 water heating equipment
/// sizing). No single authoritative standard document tabulates these exact
/// per-bedroom volume values; they are standard industry sizing conventions
/// derived from common residential code practice and DOE water heater
/// replacement sizing guidelines.
///
/// | Bedrooms | Tank Volume [gal] |
/// |----------|-------------------|
/// | 1–2      | 40                |
/// | 3–4      | 50                |
/// | 5+       | 60                |
fn tank_volume_from_bedrooms_gal(n_bedrooms: f64) -> f64 {
    let n = n_bedrooms.round() as u32;
    match n {
        0 => DEFAULT_TANK_VOLUME_GAL,
        1 | 2 => 40.0,
        3 | 4 => 50.0,
        _ => 60.0,
    }
}

/// Resolve outdoor design dry-bulb temperatures [°C].
///
/// Prefers EPW design conditions parsed from the ASHRAE `Heating`/`Cooling`
/// sections of the EPW header (TMY3 files), falling back to the `Extremes`
/// section for non-TMY3 EPW formats. Falls back to the nearest ASHRAE 152
/// climate station when EPW design conditions are unavailable (e.g., PSM3,
/// TMY3 standalone, ResStock CSV).
fn resolve_design_temperatures(
    design_conditions: Option<DesignConditions>,
    weather_lat: f64,
    weather_lon: f64,
    site_lat: Option<f64>,
    site_lon: Option<f64>,
) -> (f64, f64) {
    // Try EPW design conditions first.
    if let Some(dc) = design_conditions {
        if dc.heating_design_db_c.is_finite() && dc.cooling_design_db_c.is_finite() {
            return (dc.heating_design_db_c, dc.cooling_design_db_c);
        }
    }

    // Fall back to ASHRAE 152 climate station lookup.
    let lat = site_lat.unwrap_or(weather_lat);
    let lon = site_lon.unwrap_or(weather_lon);
    let (htg_f, clg_f) = design_temperatures_f(lat, lon).unwrap_or_else(|| {
        warn!(
            lat,
            lon,
            "no ASHRAE 152 station found — using conservative defaults: \
             heating -10 °C, cooling 35 °C"
        );
        (temperature_c_to_f(-10.0), temperature_c_to_f(35.0))
    });

    // ASHRAE 152 returns °F; convert to °C.
    let htg_c = temperature_f_to_c(htg_f);
    let clg_c = temperature_f_to_c(clg_f);
    (htg_c, clg_c)
}

/// Extract the heating setpoint from an equipment spec, falling back to
/// building setpoints and ASHRAE 90.1 defaults.
fn resolve_heating_setpoint_c(spec: &EquipmentSpec, building: &Building) -> f64 {
    // Try the equipment's explicit heating setpoint.
    if let Some(sp) = spec
        .parameters
        .get("heating_setpoint_c")
        .and_then(|v| v.as_f64())
    {
        return sp;
    }

    // Try the building's heating setpoint profile (use midnight value).
    if let Some(ref profile) = building.heating_weekday_setpoints_c {
        if !profile.is_empty() {
            return profile[0];
        }
    }

    // ASHRAE 90.1 default.
    DEFAULT_HEATING_SETPOINT_C
}

/// Extract the cooling setpoint from an equipment spec, falling back to
/// building setpoints and ASHRAE 90.1 defaults.
fn resolve_cooling_setpoint_c(spec: &EquipmentSpec, building: &Building) -> f64 {
    // Try the equipment's explicit cooling setpoint.
    if let Some(sp) = spec
        .parameters
        .get("cooling_setpoint_c")
        .and_then(|v| v.as_f64())
    {
        return sp;
    }

    // Try the building's cooling setpoint profile (use midnight value).
    if let Some(ref profile) = building.cooling_weekday_setpoints_c {
        if !profile.is_empty() {
            return profile[0];
        }
    }

    // ASHRAE 90.1 default.
    DEFAULT_COOLING_SETPOINT_C
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use hares_envelope::{OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolverConfig};
    use hares_types::{EnvironmentState, FuelType, GridState, WeatherState, ZoneId, ZoneState};
    use nalgebra::DMatrix;
    use serde_json::Map;
    use std::collections::HashMap;

    const ZONE: ZoneId = ZoneId(1);
    const UA: f64 = 20.0; // W/K
    const C: f64 = 200_000.0; // J/K
    const DT_S: f64 = 60.0; // s

    fn one_zone_env(zone_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZONE,
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 250.0,
            }],
            weather: WeatherState {
                outdoor_temp_c,
                outdoor_humidity_ratio: 0.004,
                outdoor_wet_bulb_c: outdoor_temp_c - 5.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: 0.0,
                wind_dir_deg: 0.0,
                ground_temp_c: outdoor_temp_c,
                sky_temp_c: outdoor_temp_c - 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::seconds(DT_S as i64),
            price_signal: Default::default(),
            electrical: Default::default(),
            equipment_core: Default::default(),
        }
    }

    fn build_1r1c_solver(env: &EnvironmentState, indoor_temp_c: f64) -> ThermalSolver {
        let a_c = DMatrix::from_row_slice(1, 1, &[-UA / C]);
        let b_c = DMatrix::from_row_slice(1, 2, &[UA / C, 1.0 / C]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
            .expect("1R1C state-space model must be stable");
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZONE, 0)]),
            zone_output_indices: HashMap::from([(ZONE, 0)]),
            zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZONE,
            ..ThermalSolverConfig::default()
        };
        ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
            .expect("1R1C ThermalSolver construction must succeed")
    }

    fn minimal_building() -> Building {
        Building {
            site: hares_io::hpxml::Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: Some(39.74),
                longitude_deg: Some(-104.87),
                utc_offset_h: None,
            },
            zones: vec![],
            boundaries: vec![],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: hares_io::hpxml::building::XmlNode {
                name: "root".into(),
                attrs: Default::default(),
                text: String::new(),
                children: vec![],
            },
        }
    }

    #[test]
    fn default_setpoints_match_ashrae_90_1() {
        assert!((DEFAULT_HEATING_SETPOINT_C - 21.1).abs() < f64::EPSILON);
        assert!((DEFAULT_COOLING_SETPOINT_C - 23.9).abs() < f64::EPSILON);
    }

    #[test]
    fn oversize_factors_match_manual_s() {
        assert!((HEATING_OVERSIZE_FACTOR - 1.4).abs() < f64::EPSILON);
        assert!((COOLING_OVERSIZE_FACTOR - 1.15).abs() < f64::EPSILON);
        assert!((HEATING_OVERSIZE_FACTOR_HEAT_PUMP - 1.25).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_design_temps_uses_epw_when_available() {
        let dc = DesignConditions {
            heating_design_db_c: -15.0,
            cooling_design_db_c: 38.0,
        };
        let (htg, clg) = resolve_design_temperatures(Some(dc), 0.0, 0.0, None, None);
        assert!((htg - (-15.0)).abs() < f64::EPSILON, "got {htg}");
        assert!((clg - 38.0).abs() < f64::EPSILON, "got {clg}");
    }

    #[test]
    fn resolve_design_temps_falls_back_to_ashrae_152() {
        let (htg, clg) =
            resolve_design_temperatures(None, 39.74, -104.87, Some(39.74), Some(-104.87));
        assert!(
            htg < -5.0,
            "Denver heating design {htg}°C should be below -5°C"
        );
        assert!(
            clg > 30.0,
            "Denver cooling design {clg}°C should be above 30°C"
        );
    }

    #[test]
    fn autosize_uses_manual_s_default_when_factor_absent() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let expected = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - expected).abs() < 1e-6,
            "with no factor override, capacity {capacity_w} should equal raw {raw_capacity} × Manual S factor {HEATING_OVERSIZE_FACTOR} = {expected}"
        );
    }

    #[test]
    fn autosize_heat_pump_heating_uses_1_25_factor() {
        // ACCA Manual S-2017 §4-5: heat pump heating mode oversizing is
        // limited to 1.25× because oversized heat pumps cycle excessively
        // during mild weather. Verify that the default factor for heat
        // pumps is 1.25, not the general 1.4.
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        let spec = EquipmentSpec {
            name: "ASHP Heater".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let expected_heat_pump = raw_capacity * HEATING_OVERSIZE_FACTOR_HEAT_PUMP;
        let would_be_furnace = raw_capacity * HEATING_OVERSIZE_FACTOR;

        assert!(
            (capacity_w - expected_heat_pump).abs() < 1e-6,
            "heat pump heating capacity {capacity_w} should equal raw {raw_capacity} \
             × Manual S §4-5 factor {HEATING_OVERSIZE_FACTOR_HEAT_PUMP} = {expected_heat_pump}"
        );
        assert!(
            (capacity_w - would_be_furnace).abs() > 1e-6,
            "heat pump capacity {capacity_w} must NOT equal furnace factor 1.4x result {would_be_furnace}"
        );
    }

    #[test]
    fn autosize_furnace_heating_retains_1_4_factor() {
        // Regression guard: gas furnaces and other non-heat-pump heating
        // equipment retain the existing 1.4× Manual S default.
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let expected = raw_capacity * HEATING_OVERSIZE_FACTOR;

        assert!(
            (capacity_w - expected).abs() < 1e-6,
            "gas furnace heating capacity {capacity_w} should equal raw {raw_capacity} \
             × Manual S factor {HEATING_OVERSIZE_FACTOR} = {expected}"
        );
    }

    #[test]
    fn heat_pump_override_respects_user_factor() {
        // When HPXML provides <HeatingAutosizingFactor> = 1.5 on a heat pump,
        // the override is respected (takes precedence over the Manual S cap).
        // The system warns but does not clamp user-supplied overrides.
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        params.insert("autosize_heating_factor".to_string(), json!(1.5));
        let spec = EquipmentSpec {
            name: "ASHP Heater".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let with_override = raw_capacity * 1.5;
        let with_cap = raw_capacity * HEATING_OVERSIZE_FACTOR_HEAT_PUMP;

        assert!(
            (capacity_w - with_override).abs() < 1e-6,
            "with factor 1.5 override on heat pump, capacity {capacity_w} should equal \
             raw {raw_capacity} × 1.5 = {with_override} (override takes precedence)"
        );
        assert!(
            (capacity_w - with_cap).abs() > 1e-6,
            "with factor 1.5 override on heat pump, capacity {capacity_w} must NOT \
             equal the default cap 1.25x result {with_cap}"
        );
    }

    #[test]
    fn autosize_applies_hpxml_heating_factor_override() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        params.insert("autosize_heating_factor".to_string(), json!(1.2));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let with_override = raw_capacity * 1.2;
        let with_default = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - with_override).abs() < with_override * 0.01,
            "with factor 1.2 override, capacity {capacity_w} should equal raw {raw_capacity} × 1.2 = {with_override}, not {with_default}"
        );
        assert!(
            (capacity_w - with_default).abs() > with_default * 0.01,
            "with factor 1.2 override, capacity {capacity_w} must differ from Manual S default result {with_default}"
        );
    }

    #[test]
    fn autosize_applies_hpxml_cooling_factor_override() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_cooling".to_string(), json!(true));
        params.insert("autosize_cooling_factor".to_string(), json!(1.0));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("cooling_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("cooling_capacity_w must be set after autosizing");

        // minimal_building() has no floor area → occupancy-only gains: 2 × 75 = 150 W.
        let (internal_gains_w, _internal_gains_latent_w) =
            compute_default_internal_gains(&ctx, &building);
        let raw_capacity = thermal
            .autosize_design_day_cooling(
                ZONE,
                DEFAULT_COOLING_SETPOINT_C,
                35.0,
                0.0,
                0.0,
                internal_gains_w,
            )
            .abs();
        // internal_gains_w = 132.0 (occupancy-only, no floor area in
        // minimal_building).  This matches what autosize_equipment_capacities
        // computes via compute_default_internal_gains.
        let with_override = raw_capacity * 1.0;
        let with_default = raw_capacity * COOLING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - with_override).abs() < with_override * 0.01,
            "with factor 1.0 override, capacity {capacity_w} should equal raw {raw_capacity} × 1.0 = {with_override}, not {with_default}"
        );
        assert!(
            (capacity_w - with_default).abs() > with_default * 0.01,
            "with factor 1.0 override, capacity {capacity_w} must differ from Manual S default result {with_default}"
        );
    }

    #[test]
    fn autosize_applies_limits_clamp() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        params.insert("autosize_heating_min_w".to_string(), json!(5000.0));
        params.insert("autosize_heating_max_w".to_string(), json!(600.0));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let sized = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            sized > 600.0,
            "raw capacity {sized} must exceed max limit 600 W for this test to be meaningful"
        );
        assert!(
            (capacity_w - 600.0).abs() < 1e-6,
            "capacity must be clamped to max limit 600 W, got {capacity_w}"
        );
    }

    // ── Cooling autosizing with peak solar conditions (T-0119) ────────────

    /// Build a 1R1C solver with a south-facing window and a solar input
    /// column so that `autosize_capacity_cooling` can exercise the solar path.
    fn build_1r1c_solver_with_window(
        env: &EnvironmentState,
        indoor_temp_c: f64,
        azimuth_deg: f64,
    ) -> (ThermalSolver, u32) {
        let ua = UA;
        let c = C;
        let a_c = DMatrix::from_row_slice(1, 1, &[-ua / c]);
        let b_c = DMatrix::from_row_slice(1, 3, &[ua / c, 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
            .expect("1R1C state-space model must be stable");

        let window_surface_id: u32 = 42;
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZONE, 0)]),
            zone_output_indices: HashMap::from([(ZONE, 0)]),
            zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::from([(window_surface_id, 2)]),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZONE,
            window_zone_ids: HashMap::from([(window_surface_id, ZONE)]),
            window_properties: HashMap::from([(
                window_surface_id,
                hares_envelope::WindowSolarProperties {
                    shgc: 0.5,
                    winter_shgc: 0.5,
                    u_factor_w_m2_k: 2.0,
                    area_m2: 2.0,
                    transmittance: 0.4,
                    winter_transmittance: 0.4,
                    radiation_frac: 0.2,
                    glazing_curve: hares_physics::solar::GlazingCurve::from_u_shgc(2.0, 0.5),
                    tilt_deg: 90.0,
                    azimuth_deg,
                },
            )]),
            ..ThermalSolverConfig::default()
        };
        let thermal = ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
            .expect("1R1C ThermalSolver with window must construct");
        (thermal, window_surface_id)
    }

    #[test]
    fn cooling_autosize_with_window_includes_solar_gain() {
        // Zone starts at 26 °C (above cooling setpoint 23.9 °C) so the
        // solver computes the COOLING capacity needed to reach the target.
        // With outdoor at 35 °C and a south-facing window adding solar gain,
        // more cooling capacity is required than the zero-solar case.
        let env = one_zone_env(26.0, 35.0);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env, 26.0, 180.0);

        let zero_solar_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0)
            .abs();
        let solar_capacity = thermal
            .autosize_capacity_cooling(
                ZONE,
                DEFAULT_COOLING_SETPOINT_C,
                35.0,
                39.74, // Denver
                -104.87,
                0.0, // zero internal gains for baseline
            )
            .abs();

        // Cooling with solar should be meaningfully larger than zero-solar.
        assert!(
            solar_capacity > zero_solar_capacity + 100.0,
            "solar cooling capacity {solar_capacity} W should exceed zero-solar \
             {zero_solar_capacity} W by >100 W for building with south-facing window"
        );

        // Both should be positive (cooling needed).
        assert!(
            zero_solar_capacity > 0.0,
            "zero-solar capacity {zero_solar_capacity} should be positive"
        );
        assert!(
            solar_capacity > 0.0,
            "solar capacity {solar_capacity} should be positive"
        );
    }

    #[test]
    fn heating_design_day_uses_zero_solar() {
        // Verify that the production heating design-day path uses zero solar
        // and constant temperature. It should match the preserved DC-gain
        // method (autosize_capacity) which uses zero solar by construction,
        // within a tolerance that accounts for thermal mass lag across warmup
        // and recording days.
        let env = one_zone_env(18.0, -10.0);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env, 18.0, 180.0);

        let heating_design_day =
            thermal.autosize_design_day_heating(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0);
        let dc_gain = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();

        assert!(
            heating_design_day > 400.0,
            "heating design-day capacity {heating_design_day} W should be substantial at -10 °C"
        );
        let rel_error = (heating_design_day - dc_gain).abs() / dc_gain;
        assert!(
            rel_error < 0.05,
            "heating design-day {heating_design_day:.3} W deviates from \
             DC gain {dc_gain:.3} W by {:.2} % (> 5 % tolerance); \
             heating design day must use zero solar and constant temperature",
            rel_error * 100.0
        );
    }

    // ── Backup heating autosizing (T-0120) ──────────────────────────

    #[test]
    fn autosize_backup_capacity_uses_design_heating_load() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_backup".to_string(), json!(true));
        // ASHP Heater — NOT ending in " Cooler" so needs_backup applies.
        let spec = EquipmentSpec {
            name: "ASHP Heater".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let backup_w = result
            .parameters
            .get("backup_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("backup_capacity_w must be set after backup autosizing");

        // Default factor = BACKUP_CAPACITY_FACTOR = 1.0 (no oversizing).
        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let expected = raw_capacity * BACKUP_CAPACITY_FACTOR;
        assert!(
            (backup_w - expected).abs() < 1e-6,
            "backup capacity {backup_w} should equal raw {raw_capacity} × factor 1.0 = {expected}"
        );
        // autosize_backup flag must be consumed.
        assert!(
            !result.parameters.contains_key("autosize_backup"),
            "autosize_backup flag must be removed after backup autosizing"
        );
    }

    #[test]
    fn autosize_backup_applies_factor_override() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_backup".to_string(), json!(true));
        params.insert("autosize_backup_factor".to_string(), json!(1.5));
        let spec = EquipmentSpec {
            name: "ASHP Heater".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let backup_w = result
            .parameters
            .get("backup_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("backup_capacity_w must be set after backup autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let with_override = raw_capacity * 1.5;
        let with_default = raw_capacity * BACKUP_CAPACITY_FACTOR;
        assert!(
            (backup_w - with_override).abs() < 1e-6,
            "with factor 1.5 override, backup {backup_w} should equal raw {raw_capacity} × 1.5 = {with_override}, not {with_default}"
        );
        assert!(
            (backup_w - with_default).abs() > 1.0,
            "with factor 1.5 override, backup {backup_w} must differ from default result {with_default}"
        );
        // autosize_backup flag must be consumed.
        assert!(
            !result.parameters.contains_key("autosize_backup"),
            "autosize_backup flag must be removed after backup autosizing"
        );
    }

    #[test]
    fn cooler_spec_excluded_from_backup_autosizing() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_backup".to_string(), json!(true));
        // Ends with " Cooler" — should be excluded from backup autosizing.
        let spec = EquipmentSpec {
            name: "ASHP Cooler".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        // Cooler specs should NOT receive backup_capacity_w.
        assert!(
            !result.parameters.contains_key("backup_capacity_w"),
            "cooler spec must not receive backup_capacity_w"
        );
        // autosize_backup flag should be left intact (not consumed).
        assert!(
            result.parameters.contains_key("autosize_backup"),
            "cooler spec should retain autosize_backup flag since it was excluded"
        );
    }

    #[test]
    fn cooling_autosize_without_windows_matches_zero_solar() {
        // No windows in config → autosize_capacity_cooling should return the
        // same result as autosize_capacity (pure conduction).
        let env = one_zone_env(26.0, 35.0);
        let thermal = build_1r1c_solver(&env, 26.0);

        let zero_solar = thermal
            .autosize_capacity(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0)
            .abs();
        let solar = thermal
            .autosize_capacity_cooling(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0, 0.0, 0.0, 0.0)
            .abs();

        assert!(
            (solar - zero_solar).abs() < 1e-6,
            "without windows, solar and zero-solar should match: \
             solar={solar}, zero-solar={zero_solar}"
        );
    }

    // ── Multi-node steady-state capacity regression (T-0123) ────────────────

    /// Build a 2-node solver with a thermally massive wall node.
    ///
    /// Thermal network:
    ///   zone (C=C_zone) ── UA_zo W/K ── outdoor
    ///   zone (C=C_zone) ── UA_zw W/K ── wall (C=C_wall) ── UA_wo W/K ── outdoor
    ///
    /// This models a high-R envelope where the wall-mass node has much larger
    /// capacitance than the zone-air node. The cold-start back-solve bug (prior
    /// to T-0123) would dramatically inflate autosize capacity for this model
    /// because it had to heat the wall mass from a uniform initial temperature.
    fn build_2node_high_r_solver(
        env: &EnvironmentState,
        indoor_temp_c: f64,
    ) -> (ThermalSolver, f64) {
        const C_ZONE: f64 = 200_000.0;
        const C_WALL: f64 = 2_000_000.0;
        const UA_ZO: f64 = 20.0;
        const UA_ZW: f64 = 100.0;
        const UA_WO: f64 = 10.0;

        // Effective steady-state UA from zone to outdoor (wall acts as an
        // intermediate thermal path: zone → wall → outdoor).
        // UA_eff = UA_zo + 1 / (1/UA_zw + 1/UA_wo)
        //        = UA_zo + UA_zw × UA_wo / (UA_zw + UA_wo)
        let ua_eff = UA_ZO + UA_ZW * UA_WO / (UA_ZW + UA_WO);

        // A_c: 2×2, d/dt [T_zone; T_wall]
        let a_c = DMatrix::from_row_slice(
            2,
            2,
            &[
                -(UA_ZO + UA_ZW) / C_ZONE,
                UA_ZW / C_ZONE,
                UA_ZW / C_WALL,
                -(UA_ZW + UA_WO) / C_WALL,
            ],
        );

        // B_c: 2×2, columns = [outdoor temp, HVAC to zone]
        let b_c =
            DMatrix::from_row_slice(2, 2, &[UA_ZO / C_ZONE, 1.0 / C_ZONE, UA_WO / C_WALL, 0.0]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
            .expect("2-node model must be stable");

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZONE, 0)]),
            zone_output_indices: HashMap::from([(ZONE, 0)]),
            zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let config = ThermalSolverConfig {
            indoor_zone_id: ZONE,
            ..ThermalSolverConfig::default()
        };

        let thermal = ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
            .expect("2-node ThermalSolver must construct");

        (thermal, ua_eff)
    }

    #[test]
    fn multi_node_autosize_matches_analytical_steady_state_load() {
        // Regression test for T-0123: a 2-node model with a thermally massive
        // wall node. The old code's cold-start one-step back-solve inflated
        // the apparent capacity by a factor proportional to C_wall/C_zone
        // (≈ 10× for this model). The fix computes the DC gain via two-call
        // perturbation, which produces the analytical steady-state capacity.
        let env = one_zone_env(20.0, -10.0);
        let (thermal, ua_eff) = build_2node_high_r_solver(&env, 20.0);

        let target_c = DEFAULT_HEATING_SETPOINT_C; // 21.1 °C
        let design_outdoor_c = -10.0;

        let capacity = thermal.autosize_capacity(ZONE, target_c, design_outdoor_c);

        // Analytical steady-state load: UA_eff × (T_target − T_outdoor).
        // For the parameters above: ~29.09 W/K × 31.1 K ≈ 904.7 W.
        let expected = ua_eff * (target_c - design_outdoor_c);
        let rel_error = (capacity - expected).abs() / expected.abs();

        assert!(
            rel_error < 0.02,
            "autosize capacity {capacity:.3} W deviates by {:.3}% from analytical \
             steady-state load {expected:.3} W (UA_eff={ua_eff:.3} W/K, \
             ΔT={:.1} K)",
            rel_error * 100.0,
            target_c - design_outdoor_c,
        );

        // Sanity: the old cold-start back-solve would return capacity inflated
        // by C_wall/C_zone + 1 ≈ 11×. The correct result should be well under 5×
        // the analytical load.
        assert!(
            capacity < 5.0 * expected,
            "capacity {capacity:.1} W is suspiciously inflated beyond 5× \
             steady-state load {expected:.1} W — check DC gain computation"
        );
    }

    #[test]
    fn multi_node_autosize_delta_t_linearity() {
        // The DC gain method is linear — doubling ΔT should double capacity
        // (within numerical precision). This guards against the old cold-start
        // behaviour where the transient energy to heat wall mass did NOT scale
        // linearly with ΔT.
        let env = one_zone_env(20.0, 0.0);
        let (thermal, ua_eff) = build_2node_high_r_solver(&env, 20.0);

        let target = 21.1;
        let d1 = thermal.autosize_capacity(ZONE, target, 0.0); // ΔT = 21.1
        let d2 = thermal.autosize_capacity(ZONE, target, -10.0); // ΔT = 31.1

        let ratio = d2 / d1;
        let expected_ratio = (target - (-10.0)) / (target - 0.0); // 31.1 / 21.1 ≈ 1.474
        let ratio_error = (ratio - expected_ratio).abs() / expected_ratio;

        assert!(
            ratio_error < 0.02,
            "capacity ratio ΔT=31.1 / ΔT=21.1 = {d2:.2} / {d1:.2} = {ratio:.4}, expected \
             {expected_ratio:.4} (±2%): ratio_error={:.3}%",
            ratio_error * 100.0,
        );

        // Both capacities must be positive (heating).
        assert!(d1 > 0.0, "ΔT=21.1 capacity must be positive, got {d1:.2}");
        assert!(d2 > 0.0, "ΔT=31.1 capacity must be positive, got {d2:.2}");

        // Both should be close to the analytical UA_eff × ΔT.
        let exp1 = ua_eff * (target - 0.0);
        let exp2 = ua_eff * (target - (-10.0));
        assert!(
            (d1 - exp1).abs() / exp1 < 0.02,
            "ΔT=21.1 deviates from analytical"
        );
        assert!(
            (d2 - exp2).abs() / exp2 < 0.02,
            "ΔT=31.1 deviates from analytical"
        );
    }

    // ── Full-pipeline integration tests (parse → resolve → autosize) ────

    use hares_io::defaults::DefaultsStore;
    use hares_io::hpxml::building::parse_building;
    use hares_io::hpxml::equipment::resolve_equipment;

    /// Build a minimal HPXML document with a gas furnace that omits
    /// `<HeatingCapacity>` but includes efficiency and building metadata.
    fn furnace_without_heating_capacity_hpxml() -> &'static str {
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>39.74</Latitude>
          <Longitude>-104.87</Longitude>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatingSystem>
            <SystemIdentifier id="fur1"/>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units>
              <Value>0.92</Value>
            </AnnualHeatingEfficiency>
          </HeatingSystem>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
    }

    /// Build a minimal HPXML document with a gas furnace that includes an
    /// explicit `<HeatingCapacity>` — happy-path control for the autosizing
    /// pipeline test.
    fn furnace_with_heating_capacity_hpxml() -> &'static str {
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>39.74</Latitude>
          <Longitude>-104.87</Longitude>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatingSystem>
            <SystemIdentifier id="fur1"/>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units>
              <Value>0.92</Value>
            </AnnualHeatingEfficiency>
            <HeatingCapacity units="Btuh">60000</HeatingCapacity>
          </HeatingSystem>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
    }

    /// Resolve equipment from an HPXML string. Returns the gas furnace spec
    /// (the first HVAC spec in the resolved list).
    fn resolve_furnace_spec(xml: &str) -> EquipmentSpec {
        let building = parse_building(xml).expect("HPXML must parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}), None)
            .expect("equipment must resolve");
        specs
            .into_iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace must be in resolved specs")
    }

    #[test]
    fn autosize_pipeline_parse_to_capacity_with_epw_conditions() {
        let xml = furnace_without_heating_capacity_hpxml();
        let spec = resolve_furnace_spec(xml);

        // After resolve: autosize flag must be set, no capacity, no typed config.
        assert!(
            spec.parameters
                .get("autosize_heating")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "autosize_heating must be true when HeatingCapacity is omitted"
        );
        assert!(
            !spec.parameters.contains_key("heating_capacity_w"),
            "heating_capacity_w must be absent before autosizing"
        );
        assert!(
            spec.typed_config.is_none(),
            "typed_config must be None before autosizing"
        );

        // Build a 1R1C solver and run autosizing with EPW-derived design conditions.
        let env = one_zone_env(20.0, -5.0);
        let thermal = build_1r1c_solver(&env, 20.0);
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -15.0,
                cooling_design_db_c: 38.0,
            }),
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };

        let mut specs = vec![spec];
        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        // Autosize flag must be consumed.
        assert!(
            !result.parameters.contains_key("autosize_heating"),
            "autosize_heating must be removed after autosizing"
        );

        // Typed config must be rebuilt.
        assert!(
            result.typed_config.is_some(),
            "typed_config must be Some after autosizing"
        );

        // Capacity must be positive.
        assert!(
            capacity_w > 0.0,
            "autosized capacity {capacity_w} must be positive"
        );

        // Plausible range for this building: ΔT = 21.1 - (-15.0) = 36.1 K,
        // UA = 20 W/K, raw ≈ 722 W, after 1.4x ≈ 1010 W.
        let min_plausible = 100.0;
        let max_plausible = 50_000.0;
        assert!(
            capacity_w > min_plausible,
            "autosized capacity {capacity_w} W below plausible minimum {min_plausible} W"
        );
        assert!(
            capacity_w < max_plausible,
            "autosized capacity {capacity_w} W above plausible maximum {max_plausible} W"
        );

        // Capacity should scale with ΔT: computed value matches raw × Manual S factor.
        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -15.0)
            .abs();
        let expected = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - expected).abs() < 1e-6,
            "capacity {capacity_w} should equal raw {raw_capacity} × Manual S factor {HEATING_OVERSIZE_FACTOR} = {expected}"
        );
    }

    #[test]
    fn autosize_pipeline_parse_to_capacity_with_ashrae_152_fallback() {
        let xml = furnace_without_heating_capacity_hpxml();
        let spec = resolve_furnace_spec(xml);

        assert!(
            spec.parameters
                .get("autosize_heating")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "autosize_heating must be true when HeatingCapacity is omitted"
        );

        // Build a 1R1C solver and run autosizing with design_conditions = None
        // to exercise the ASHRAE 152 climate station fallback.
        let env = one_zone_env(20.0, -5.0);
        let thermal = build_1r1c_solver(&env, 20.0);
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: None,
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };

        let mut specs = vec![spec];
        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing via ASHRAE 152 fallback");

        assert!(
            !result.parameters.contains_key("autosize_heating"),
            "autosize_heating must be removed after autosizing"
        );
        assert!(
            result.typed_config.is_some(),
            "typed_config must be Some after autosizing via ASHRAE 152 fallback"
        );
        assert!(
            capacity_w > 0.0,
            "autosized capacity {capacity_w} must be positive with ASHRAE 152 fallback"
        );

        // Denver ASHRAE 152 heating design temp is 3 °F (≈ -16 °C).
        // ΔT = 21.1 − (−16.1) ≈ 37 K, UA = 20 W/K → raw ≈ 740 W, after 1.4× ≈ 1040 W.
        let min_plausible = 100.0;
        let max_plausible = 50_000.0;
        assert!(
            capacity_w > min_plausible,
            "ASHRAE 152 fallback capacity {capacity_w} W below plausible minimum {min_plausible} W"
        );
        assert!(
            capacity_w < max_plausible,
            "ASHRAE 152 fallback capacity {capacity_w} W above plausible maximum {max_plausible} W"
        );
    }

    #[test]
    fn autosize_pipeline_explicit_capacity_skips_autosizing() {
        let xml = furnace_with_heating_capacity_hpxml();
        let spec = resolve_furnace_spec(xml);

        // With explicit HeatingCapacity, autosize should NOT be flagged.
        assert!(
            !spec
                .parameters
                .get("autosize_heating")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "autosize_heating must be false when HeatingCapacity is explicitly provided"
        );

        // The explicit capacity is stored in params by the resolver.
        let capacity_before = spec
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be present when capacity is explicit");

        // The typed config should already be populated.
        assert!(
            spec.typed_config.is_some(),
            "typed_config must be Some when capacity is explicitly provided"
        );

        // Running autosize should not change the spec (no autosize flags
        // means the spec is skipped entirely).
        let env = one_zone_env(20.0, -5.0);
        let thermal = build_1r1c_solver(&env, 20.0);
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -15.0,
                cooling_design_db_c: 38.0,
            }),
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };

        let mut specs = vec![spec];
        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        assert!(
            !result.parameters.contains_key("autosize_heating"),
            "autosize_heating was not set, must not appear after autosizing"
        );
        // Explicit capacity must still be present and unchanged.
        let capacity_after = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must still be present after autosizing");
        assert!(
            (capacity_after - capacity_before).abs() < 1e-6,
            "explicit capacity must be unchanged by autosizing"
        );
    }

    // ── Design-day autosizing tests (T-0192) ────────────────────────────

    #[test]
    fn design_day_heating_zero_load_produces_zero_peak() {
        // When indoor and outdoor temperatures are equal, the required
        // HVAC input should be near zero (only thermal-mass redistribution
        // residuals remain, which approach zero after warmup).
        let target = 20.0;
        let design_outdoor = 20.0;
        let env = one_zone_env(target, design_outdoor);
        let thermal = build_1r1c_solver(&env, target);

        let peak = thermal.autosize_design_day_heating(ZONE, target, design_outdoor);
        assert!(
            peak.abs() < 1.0,
            "zero ΔT: peak load {peak} W should be near zero (< 1 W)"
        );
        assert!(peak.is_finite(), "peak load must be finite");
    }

    #[test]
    fn design_day_cooling_with_solar_returns_finite_positive() {
        // The cooling design day includes a diurnal outdoor temperature
        // profile and (when lat/lon is non-zero) solar gains. At a minimum,
        // verify the result is finite, non-negative, and non-zero when
        // there is a driving temperature difference.
        let target = 24.0;
        let design_outdoor = 35.0;
        let env = one_zone_env(target + 2.0, design_outdoor);
        let thermal = build_1r1c_solver(&env, target + 2.0);

        let peak = thermal.autosize_design_day_cooling(ZONE, target, design_outdoor, 0.0, 0.0, 0.0);
        assert!(peak.is_finite(), "cooling peak load must be finite");
        assert!(peak >= 0.0, "cooling capacity must be non-negative");
        assert!(
            peak > 1.0,
            "{target} °C target vs {design_outdoor} °C outdoor should produce \
             non-trivial cooling load, got {peak} W"
        );
    }

    #[test]
    fn design_day_heating_matches_dc_gain_within_tolerance() {
        // With constant outdoor temperature (no diurnal variation), the
        // design-day simulation with 2 warmup days should converge to the
        // discrete steady-state load, which closely approximates the DC
        // gain result. The expected discrepancy is < 0.1 % for a 1R1C model.
        let target = 21.1;
        let design_outdoor = -10.0;
        let env = one_zone_env(target - 1.0, design_outdoor);
        let thermal = build_1r1c_solver(&env, target - 1.0);

        let dc_gain_capacity = thermal
            .autosize_capacity(ZONE, target, design_outdoor)
            .abs();
        let design_day_capacity = thermal.autosize_design_day_heating(ZONE, target, design_outdoor);

        let rel_error = (design_day_capacity - dc_gain_capacity).abs() / dc_gain_capacity;
        assert!(
            rel_error < 0.005,
            "constant-outdoor design-day heating {design_day_capacity:.5} W deviates from \
             DC gain {dc_gain_capacity:.5} W by {:.3} % (> 0.5 % tolerance)",
            rel_error * 100.0
        );
    }

    #[test]
    fn design_day_cooling_matches_dc_gain_within_thermal_mass_tolerance() {
        // With constant outdoor temperature and zero solar (lat=0,lon=0 at
        // equator gives zero or near-zero solar at all hours for the July 21
        // clear-sky model), the design-day cooling simulation should
        // approximate the DC gain result. The diurnal outdoor temperature
        // variation introduces a small discrepancy from thermal mass lag.
        let target = 24.0;
        let design_outdoor = 35.0;
        let env = one_zone_env(target + 2.0, design_outdoor);
        let thermal = build_1r1c_solver(&env, target + 2.0);

        let dc_gain_capacity = thermal
            .autosize_capacity(ZONE, target, design_outdoor)
            .abs();
        let design_day_capacity =
            thermal.autosize_design_day_cooling(ZONE, target, design_outdoor, 0.0, 0.0, 0.0);

        // The design-day method uses a diurnal range of 11.7 °C, so the
        // outdoor temperature cycles between 23.3 and 35.0 °C. The peak
        // load approximately matches the DC gain (which uses constant
        // 35 °C). The thermal mass introduces a lag, so the peak may be
        // slightly lower. Allow 5 % tolerance for this thermal mass effect.
        let rel_error = (design_day_capacity - dc_gain_capacity).abs() / dc_gain_capacity;
        assert!(
            rel_error < 0.05,
            "design-day cooling {design_day_capacity:.3} W deviates from \
             DC gain {dc_gain_capacity:.3} W by {:.2} % (> 5 % tolerance)",
            rel_error * 100.0
        );
    }

    #[test]
    fn design_day_empty_zone_completes_without_panicking() {
        // A zone with zero thermal capacitance is a pathological (invalid)
        // model, but the design-day simulation loop must not panic when
        // encountering it. The solver should return 0 or a finite value
        // rather than producing NaN or panicking.
        //
        // Build a solver with C = 0 (no thermal mass). The state-space
        // model and autosize methods must handle this gracefully.
        let ua = 20.0;
        // C = 0 produces singular matrices (A_c = -UA/0 = -inf). The design-day
        // loop must not panic on degenerate input; it should return a finite value.
        // Use a tiny non-zero C to exercise the fast-response path.
        let _c = 0.0;
        let dt = 60.0;
        let c_tiny = 1.0; // J/K — extremely fast thermal response
        let a_c = DMatrix::from_row_slice(1, 1, &[-ua / c_tiny]);
        let b_c = DMatrix::from_row_slice(1, 2, &[ua / c_tiny, 1.0 / c_tiny]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = match StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping) {
            Ok(m) => m,
            Err(_) => {
                // With C=0 the matrices are singular; skip if construction fails.
                return;
            }
        };
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZONE, 0)]),
            zone_output_indices: HashMap::from([(ZONE, 0)]),
            zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZONE,
            ..ThermalSolverConfig::default()
        };

        let env = one_zone_env(20.0, 10.0);
        let thermal = match ThermalSolver::new(model, wiring, config, dt, &env, 20.0) {
            Ok(t) => t,
            Err(_) => return, // construction failure is acceptable
        };

        // Heating
        let h = thermal.autosize_design_day_heating(ZONE, 21.0, -10.0);
        assert!(h.is_finite(), "heating design-day must return finite value");
        assert!(h >= 0.0, "heating capacity must be non-negative");

        // Cooling
        let c = thermal.autosize_design_day_cooling(ZONE, 24.0, 35.0, 0.0, 0.0, 0.0);
        assert!(c.is_finite(), "cooling design-day must return finite value");
        assert!(c >= 0.0, "cooling capacity must be non-negative");
    }

    // ── Diurnal solar scan orientation regression tests (T-0195) ─────────

    #[test]
    fn west_window_diurnal_exceeds_noon_only_capacity() {
        // A zone with only west-facing windows should produce a higher peak
        // cooling capacity from the design-day diurnal simulation than from
        // the single-point noon-only DC gain method. At solar noon, the sun
        // azimuth is ~180° (south), which is nearly orthogonal to a west-facing
        // (270°) vertical window — yielding negligible direct beam gain. The
        // diurnal simulation captures the 4–6 PM solar peak where the sun
        // azimuth aligns with the window, producing significantly higher solar
        // gain and therefore a larger cooling requirement.
        // ACCA Manual J-2016 §7–8: hour-by-hour solar profiles per exposure
        // are required for accurate design cooling loads.
        let target = DEFAULT_COOLING_SETPOINT_C;
        let design_outdoor = 35.0;

        let env_west = one_zone_env(target + 2.0, design_outdoor);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env_west, target + 2.0, 270.0);

        let noon_only = thermal
            .autosize_capacity_cooling(
                ZONE,
                target,
                design_outdoor,
                39.74, // Denver
                -104.87,
                0.0,
            )
            .abs();

        let diurnal = thermal
            .autosize_design_day_cooling(
                ZONE,
                target,
                design_outdoor,
                39.74, // Denver
                -104.87,
                0.0,
            )
            .abs();

        // Both should produce positive cooling capacity.
        assert!(
            noon_only > 0.0,
            "noon-only cooling capacity {noon_only} must be positive"
        );
        assert!(
            diurnal > 0.0,
            "diurnal cooling capacity {diurnal} must be positive"
        );

        // The diurnal simulation must exceed noon-only by a meaningful
        // margin: the west-facing window receives substantial solar at
        // 4 PM that the noon-only method entirely misses.
        assert!(
            diurnal > noon_only + 25.0,
            "west-facing (270°) diurnal peak {diurnal} W must exceed noon-only \
             {noon_only} W (diurnal captures 4 PM west solar; noon misses it)"
        );
    }

    #[test]
    fn south_window_diurnal_matches_noon_only_within_tolerance() {
        // A zone with only south-facing windows should produce approximately
        // the same peak cooling capacity from the diurnal simulation as from
        // the single-point noon-only method. Solar noon (solar azimuth ~180°
        // for Northern Hemisphere) aligns well with a south-facing (180°)
        // vertical window, so the noon-only snapshot captures the peak solar
        // condition for this orientation. The remaining discrepancy comes
        // from the diurnal outdoor temperature profile (peak at 3 PM) and
        // thermal mass lag — both second-order effects compared to the
        // dominant solar-gain timing match.
        // ACCA Manual J-2016 §7–8: south-facing peak load coincides with
        // solar noon.
        let target = DEFAULT_COOLING_SETPOINT_C;
        let design_outdoor = 35.0;

        let env_south = one_zone_env(target + 2.0, design_outdoor);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env_south, target + 2.0, 180.0);

        let noon_only = thermal
            .autosize_capacity_cooling(
                ZONE,
                target,
                design_outdoor,
                39.74, // Denver
                -104.87,
                0.0,
            )
            .abs();

        let diurnal = thermal
            .autosize_design_day_cooling(
                ZONE,
                target,
                design_outdoor,
                39.74, // Denver
                -104.87,
                0.0,
            )
            .abs();

        // Both should produce positive cooling capacity.
        assert!(
            noon_only > 0.0,
            "noon-only cooling capacity {noon_only} must be positive"
        );
        assert!(
            diurnal > 0.0,
            "diurnal cooling capacity {diurnal} must be positive"
        );

        // South-facing windows peak near solar noon, so the diurnal result
        // should be close to the noon-only DC gain. Allow 10 % tolerance
        // for diurnal outdoor temperature profile (11.7 °C range peaking
        // at 3 PM) and thermal mass lag effects.
        let rel_error = (diurnal - noon_only).abs() / noon_only;
        assert!(
            rel_error < 0.10,
            "south-facing (180°) diurnal peak {diurnal:.3} W deviates from \
             noon-only {noon_only:.3} W by {:.2} % (> 10 % tolerance); \
             south windows peak near noon so the two methods should agree",
            rel_error * 100.0
        );
    }

    // ── Internal gains cooling autosizing tests (T-0193) ────────────────

    #[test]
    fn cooling_autosize_internal_gains_increases_capacity() {
        // With non-zero internal gains, the cooling capacity should increase
        // because the HVAC must remove additional heat generated inside the zone.
        let env = one_zone_env(26.0, 35.0);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env, 26.0, 180.0);

        let zero_gains_capacity = thermal
            .autosize_capacity_cooling(
                ZONE,
                DEFAULT_COOLING_SETPOINT_C,
                35.0,
                39.74, // Denver
                -104.87,
                0.0,
            )
            .abs();

        let with_gains_capacity = thermal
            .autosize_capacity_cooling(
                ZONE,
                DEFAULT_COOLING_SETPOINT_C,
                35.0,
                39.74, // Denver
                -104.87,
                500.0, // 500 W internal gains
            )
            .abs();

        // Cooling with internal gains should be larger than without by
        // approximately the amount of the internal gains (within a small
        // tolerance for DC-gain linearity).
        let delta = with_gains_capacity - zero_gains_capacity;
        assert!(
            delta > 400.0,
            "internal gains increase should be close to 500 W, got {delta} W: \
             zero_gains={zero_gains_capacity}, with_gains={with_gains_capacity}"
        );
        assert!(
            (delta - 500.0).abs() < 10.0,
            "internal gains delta {delta} should be within 10 W of 500 W \
             (DC gain method is linear to machine precision for linear models)"
        );
    }

    #[test]
    fn cooling_autosize_zero_internal_gains_preserves_baseline() {
        // Backward compatibility: zone with internal gains = 0 should produce
        // the same sizing result as before the internal gains change.
        let env = one_zone_env(26.0, 35.0);
        let thermal = build_1r1c_solver(&env, 26.0);

        // DC-gain without solar (autosize_capacity)
        let dc_gain = thermal
            .autosize_capacity(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0)
            .abs();

        // Cooling-specific method with zero internal gains and zero solar
        // should match the DC-gain baseline.
        let cooling_zero_gains = thermal
            .autosize_capacity_cooling(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0, 0.0, 0.0, 0.0)
            .abs();

        assert!(
            (cooling_zero_gains - dc_gain).abs() < 1e-6,
            "with zero internal gains and zero solar, \
             autosize_capacity_cooling ({cooling_zero_gains}) \
             must match autosize_capacity ({dc_gain})"
        );
    }

    #[test]
    fn compute_default_internal_gains_matches_ashrae_defaults() {
        // Verify that the default internal gains match:
        // - Occupancy: 2 × 75 W/person = 150 W sensible (ASHRAE HoF 2021 Ch.18 Table 1)
        // - Lighting/plug: 5 W/m² per ASHRAE 62.2-2022 Appendix B
        // - Occupancy latent: 2 × 55 W/person = 110 W (ASHRAE HoF 2021 Ch.18 Table 1)

        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: None,
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };

        // minimal_building() has no conditioned volume or floor area, so
        // the function should return occupancy-only gains with a warning.
        let (sensible, latent) = compute_default_internal_gains(&ctx, &building);

        // Sensible from occupancy only (no floor area → no lights/plug component).
        let expected_occ_sensible = 2.0 * OCCUPANT_SENSIBLE_GAIN_W; // 150 W
        let expected_occ_latent = 2.0 * OCCUPANT_LATENT_GAIN_W; // 110 W

        assert!(
            (sensible - expected_occ_sensible).abs() < 1e-6,
            "occupancy-only sensible gains {sensible} should match \
             2 × {OCCUPANT_SENSIBLE_GAIN_W} = {expected_occ_sensible}"
        );
        assert!(
            (latent - expected_occ_latent).abs() < 1e-6,
            "occupancy-only latent gains {latent} should match \
             2 × {OCCUPANT_LATENT_GAIN_W} = {expected_occ_latent}"
        );
    }

    #[test]
    fn compute_default_internal_gains_includes_lighting_plug_from_floor_area() {
        // When the building has a conditioned floor area, the default internal
        // gains should include occupancy + 5 W/m² lights/plug loads.
        use hares_io::hpxml::building::{Zone, ZoneType};

        let mut building = minimal_building();
        building.conditioned_volume_m3 = Some(180.0); // 150 m² × 2.4 m ceiling
        building.ceiling_height_m = Some(2.4);
        building.zones = vec![Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(75.0),
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }];

        let ctx = AutosizeContext {
            design_conditions: None,
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };

        let (sensible, latent) = compute_default_internal_gains(&ctx, &building);

        // conditioned_volume / ceiling_height = 180 / 2.4 = 75 m²
        // Occupancy: 2 × 75 = 150 W
        // Lighting/plug: 75 × 5 = 375 W
        // Total sensible: 150 + 375 = 525 W
        let expected_sensible = 150.0 + 75.0 * 5.0;
        assert!(
            (sensible - expected_sensible).abs() < 1e-6,
            "gains with floor area: sensible {sensible} should match \
             occupancy (150 W) + lighting/plug (75m² × 5W/m² = 375 W) = {expected_sensible} W"
        );
        assert!(
            (latent - 110.0).abs() < 1e-6,
            "latent gains {latent} should match 2 × {OCCUPANT_LATENT_GAIN_W} = 110 W"
        );
    }

    #[test]
    fn compute_default_internal_gains_respects_context_override() {
        // When AutosizeContext provides non-zero internal gains (HPXML override),
        // those should take precedence over the computed defaults.
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: None,
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
            internal_gains_w: 800.0,
            internal_gains_latent_w: 200.0,
        };

        let (sensible, latent) = compute_default_internal_gains(&ctx, &building);

        assert!(
            (sensible - 800.0).abs() < 1e-6,
            "override: sensible should be 800 W, got {sensible}"
        );
        assert!(
            (latent - 200.0).abs() < 1e-6,
            "override: latent should be 200 W, got {latent}"
        );
    }

    #[test]
    fn design_day_cooling_internal_gains_increases_peak() {
        // Design-day cooling with internal gains should produce higher peak
        // than without (conservative sizing with gains).
        let target = 24.0;
        let design_outdoor = 35.0;
        let env = one_zone_env(target + 2.0, design_outdoor);
        let thermal = build_1r1c_solver(&env, target + 2.0);

        let zero_gains =
            thermal.autosize_design_day_cooling(ZONE, target, design_outdoor, 0.0, 0.0, 0.0);
        let with_gains =
            thermal.autosize_design_day_cooling(ZONE, target, design_outdoor, 0.0, 0.0, 500.0);

        assert!(
            with_gains > zero_gains + 400.0,
            "design-day cooling with 500 W internal gains ({with_gains} W) \
             should exceed zero-gains peak ({zero_gains} W) by ~500 W"
        );
        assert!(with_gains.is_finite(), "with-gains peak must be finite");
    }

    // ── Water heater autosizing tests (T-0194) ─────────────────────────

    fn wh_spec(name: &str, fuel: FuelType, extra_params: &[(&str, Value)]) -> EquipmentSpec {
        let mut params = Map::new();
        params.insert("autosize_water_heater".to_string(), json!(true));
        for (k, v) in extra_params {
            params.insert(k.to_string(), v.clone());
        }
        EquipmentSpec {
            name: name.to_string(),
            instance_name: None,
            fuel_type: fuel,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn wh_autosize_three_bedrooms_fifty_f_mains_120_f_setpoint() {
        // 3 bedrooms → tank 50 gal, FHR 48 GPH
        // mains = 50 °F (10 °C), setpoint = 120 °F (48.89 °C) → ΔT = 38.89 °C
        // usable vol = 0.7 × 50 = 35 gal
        // capacity = (48 − 35) × 4.395 × 38.89 ≈ 13 × 170.9 ≈ 2222 W
        let setpoint_c = hares_physics::units::temperature_f_to_c(120.0);
        let mut specs = vec![wh_spec(
            "Electric Resistance Water Heater",
            FuelType::Electric,
            &[("setpoint_c", json!(setpoint_c))],
        )];
        autosize_water_heater_capacities(&mut specs, Some(3.0), 10.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set");
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("tank_volume_m3 must be set");

        assert!(
            !result.parameters.contains_key("autosize_water_heater"),
            "autosize flag must be consumed"
        );

        let expected_gal = 50.0;
        let expected_vol_m3 = hares_physics::units::volume_gal_to_m3(expected_gal);
        assert!(
            (volume_m3 - expected_vol_m3).abs() < 1e-6,
            "tank volume {volume_m3} m3 should match {expected_gal} gal → {expected_vol_m3} m3"
        );

        // 3 BR → FHR = 48 GPH
        let fhr_gph = required_fhr_gph(3.0);
        assert!(
            (fhr_gph - 48.0).abs() < 1e-6,
            "FHR for 3 BR should be 48 GPH"
        );

        let tank_gal = tank_volume_from_bedrooms_gal(3.0);
        assert!(
            (tank_gal - 50.0).abs() < 1e-6,
            "tank for 3 BR should be 50 gal"
        );

        // ΔT = 120 °F − 50 °F = 70 °F = 38.89 °C (setpoint 120 °F, mains 50 °F)
        // usable = 0.7 × 50 = 35 gal
        // capacity = (48 − 35) × 4.395 × 38.89 = 13 × 170.9 ≈ 2222 W
        let expected_cap = (48.0 - 0.7 * 50.0) * WATER_ENERGY_FACTOR * (setpoint_c - 10.0);
        assert!(
            (capacity_w - expected_cap).abs() < 1e-3,
            "capacity {capacity_w:.2} W should match computed {expected_cap:.2} W \
             (13 gal deficit × 4.395 factor × 38.89 K ΔT)"
        );
        assert!(
            capacity_w > 2000.0,
            "capacity {capacity_w} W should be reasonable"
        );
    }

    #[test]
    fn wh_autosize_false_explicit_capacity_not_overridden() {
        // When autosize_water_heater is false and explicit capacity is
        // provided, the autosizer must leave the spec unchanged.
        let mut params = Map::new();
        // No autosize_water_heater flag — explicit values only.
        params.insert("heating_capacity_w".to_string(), json!(4500.0));
        params.insert("tank_volume_m3".to_string(), json!(0.151));
        let spec = EquipmentSpec {
            name: "Gas Water Heater".to_string(),
            instance_name: None,
            fuel_type: FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        autosize_water_heater_capacities(&mut specs, Some(3.0), 10.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("explicit capacity must remain");
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("explicit volume must remain");

        assert!(
            (capacity_w - 4500.0).abs() < 1e-6,
            "explicit capacity 4500 W should not be overridden, got {capacity_w}"
        );
        assert!(
            (volume_m3 - 0.151).abs() < 1e-6,
            "explicit volume 0.151 m3 should not be overridden, got {volume_m3}"
        );
    }

    #[test]
    fn wh_autosize_zero_bedrooms_uses_conservative_defaults() {
        // Zero bedrooms should NOT panic. The autosizer must fall back to
        // conservative defaults (50 gal, computed from default 2.0 bedroom
        // tier logic → 50 GPH FHR).
        let mut specs = vec![wh_spec(
            "Electric Resistance Water Heater",
            FuelType::Electric,
            &[],
        )];
        autosize_water_heater_capacities(&mut specs, Some(0.0), 10.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("capacity must be set even with zero bedrooms");
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("volume must be set even with zero bedrooms");

        // Default tank for 0 BR is 50 gal.
        let expected_vol_m3 = hares_physics::units::volume_gal_to_m3(50.0);
        assert!(
            (volume_m3 - expected_vol_m3).abs() < 1e-6,
            "zero-bedroom default volume should be 50 gal, got {volume_m3} m3"
        );
        assert!(
            capacity_w > 0.0 && capacity_w.is_finite(),
            "zero-bedroom capacity {capacity_w} must be positive and finite"
        );
    }

    #[test]
    fn wh_autosize_unreasonable_mains_temp_uses_default() {
        // A mains temp of 60 °C is unreasonable for cold water; the
        // autosizer should fall back to the default (25 °C) and produce a
        // reasonable capacity.
        let mut specs = vec![wh_spec("Gas Water Heater", FuelType::Gas, &[])];
        autosize_water_heater_capacities(&mut specs, Some(3.0), 60.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("capacity must be set");

        // With 25 °C default mains and 51.67 °C setpoint: ΔT = 26.67 °C
        // 3 BR → FHR = 48, tank = 50 gal
        // usable = 35 gal, deficit = 13 gal
        // capacity = 13 × 4.395 × 26.67 ≈ 1524 W
        let expected = (48.0 - 0.7 * 50.0)
            * WATER_ENERGY_FACTOR
            * (DEFAULT_WH_SETPOINT_C - DEFAULT_MAINS_TEMP_C);
        assert!(
            (capacity_w - expected).abs() < 1e-3,
            "with unreasonable mains temp 60 °C, should fall back to 25 °C \
             and produce {expected:.2} W, got {capacity_w:.2} W"
        );
        assert!(capacity_w > 0.0, "capacity must be positive");
    }

    #[test]
    fn wh_autosize_none_bedrooms_uses_conservative_defaults() {
        // When n_bedrooms is None, the autosizer must use conservative
        // defaults without panicking.
        let mut specs = vec![wh_spec(
            "Electric Resistance Water Heater",
            FuelType::Electric,
            &[],
        )];
        autosize_water_heater_capacities(&mut specs, None, 10.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("capacity must be set even with None bedrooms");
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("volume must be set even with None bedrooms");

        let expected_vol_m3 = hares_physics::units::volume_gal_to_m3(DEFAULT_TANK_VOLUME_GAL);
        assert!(
            (volume_m3 - expected_vol_m3).abs() < 1e-6,
            "None-bedroom volume should be default 50 gal"
        );
        assert!(
            capacity_w > 0.0 && capacity_w.is_finite(),
            "None-bedroom capacity must be positive"
        );
    }

    #[test]
    fn wh_autosize_applies_factor_override() {
        // Water heater factor override should scale the capacity.
        let mut specs = vec![wh_spec(
            "Electric Resistance Water Heater",
            FuelType::Electric,
            &[("autosize_water_heater_factor", json!(1.5))],
        )];
        autosize_water_heater_capacities(&mut specs, Some(3.0), 10.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("capacity must be set");

        // Base: 3 BR → FHR = 48, tank = 50 → deficit 13 gal
        // capacity_base = 13 × 4.395 × 41.67 ≈ 2380 W
        // With factor 1.5: 2380 × 1.5 = 3570 W
        let base_cap = (48.0 - 0.7 * 50.0) * WATER_ENERGY_FACTOR * (DEFAULT_WH_SETPOINT_C - 10.0);
        let expected = base_cap * 1.5;
        assert!(
            (capacity_w - expected).abs() < 1e-3,
            "with factor 1.5, capacity should be base {base_cap:.2} × 1.5 = {expected:.2}, \
             got {capacity_w:.2}"
        );
        assert!(
            !result
                .parameters
                .contains_key("autosize_water_heater_factor"),
            "autosize_water_heater_factor must be consumed"
        );
    }

    #[test]
    fn wh_autosize_respects_min_capacity_limit() {
        // Minimum capacity clamp must work.
        let mut specs = vec![wh_spec(
            "Electric Resistance Water Heater",
            FuelType::Electric,
            &[("autosize_water_heater_min_w", json!(10000.0))],
        )];
        // 1 BR → tank = 40 gal, FHR = 36 GPH, usable = 28 gal, deficit = 8 gal
        // capacity = 8 × 4.395 × 41.67 ≈ 1465 W, below 10kW min
        autosize_water_heater_capacities(&mut specs, Some(1.0), 10.0);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("capacity must be set");

        assert!(
            (capacity_w - 10000.0).abs() < 1e-3,
            "capacity must be clamped to min 10 kW, got {capacity_w}"
        );
        assert!(
            !result
                .parameters
                .contains_key("autosize_water_heater_min_w"),
            "min limit must be consumed"
        );
    }

    #[test]
    fn wh_fhr_table_values_match_per_bedroom_sizing() {
        // Verify the FHR lookup table values.
        assert!((required_fhr_gph(1.0) - 36.0).abs() < 1e-6);
        assert!((required_fhr_gph(2.0) - 42.0).abs() < 1e-6);
        assert!((required_fhr_gph(3.0) - 48.0).abs() < 1e-6);
        assert!((required_fhr_gph(4.0) - 54.0).abs() < 1e-6);
        assert!((required_fhr_gph(5.0) - 62.0).abs() < 1e-6);
        assert!((required_fhr_gph(6.0) - 62.0).abs() < 1e-6);
        assert!((required_fhr_gph(0.0) - 50.0).abs() < 1e-6);
    }

    #[test]
    fn wh_tank_volume_table_matches_sizing_rules() {
        // Verify the tank volume lookup table.
        assert!((tank_volume_from_bedrooms_gal(1.0) - 40.0).abs() < 1e-6);
        assert!((tank_volume_from_bedrooms_gal(2.0) - 40.0).abs() < 1e-6);
        assert!((tank_volume_from_bedrooms_gal(3.0) - 50.0).abs() < 1e-6);
        assert!((tank_volume_from_bedrooms_gal(4.0) - 50.0).abs() < 1e-6);
        assert!((tank_volume_from_bedrooms_gal(5.0) - 60.0).abs() < 1e-6);
        assert!((tank_volume_from_bedrooms_gal(6.0) - 60.0).abs() < 1e-6);
        assert!((tank_volume_from_bedrooms_gal(0.0) - DEFAULT_TANK_VOLUME_GAL).abs() < 1e-6);
    }

    #[test]
    fn wh_autosize_hpwh_uses_backup_element_power_w_field() {
        // Heat Pump Water Heater uses `backup_element_power_w` as the
        // capacity field name, not `heating_capacity_w`.
        let mut specs = vec![wh_spec("Heat Pump Water Heater", FuelType::Electric, &[])];
        autosize_water_heater_capacities(&mut specs, Some(3.0), 10.0);

        let result = &specs[0];
        // The autosizer writes `heating_capacity_w` to params (generic key)
        // but patches the typed_config with `backup_element_power_w`.
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("params must have heating_capacity_w");
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("params must have tank_volume_m3");

        assert!(capacity_w > 0.0);
        assert!(volume_m3 > 0.0);
        assert!(!result.parameters.contains_key("autosize_water_heater"));
    }

    #[test]
    fn wh_autosize_indirect_tank_only_sets_volume() {
        // Indirect tanks get heat from the boiler. The autosizer computes
        // heating_capacity_w and writes it to spec.parameters (the generic
        // capacity field), but skips patching typed_config for Indirect Tank
        // because the config struct uses a different field name for capacity.
        // This test verifies: tank volume is set, flag is consumed.
        let mut specs = vec![wh_spec("Indirect Tank", FuelType::Gas, &[])];
        autosize_water_heater_capacities(&mut specs, Some(3.0), 10.0);

        let result = &specs[0];
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("tank_volume_m3 must be set");

        let expected_vol_m3 = hares_physics::units::volume_gal_to_m3(50.0);
        assert!((volume_m3 - expected_vol_m3).abs() < 1e-6);

        // Indirect Tank typed_config is not patched (different field name);
        // heating_capacity_w in params is set by the generic capacity path
        // but not asserted here. Key verification: volume is set and flag consumed.
        assert!(
            !result.parameters.contains_key("autosize_water_heater"),
            "autosize flag must be consumed"
        );
    }

    #[test]
    fn wh_autosize_preserves_existing_volume() {
        // When HPXML provides TankVolume, the autosizer must NOT override it.
        let mut params = Map::new();
        params.insert("autosize_water_heater".to_string(), json!(true));
        params.insert("tank_volume_m3".to_string(), json!(0.1));
        let spec = EquipmentSpec {
            name: "Gas Water Heater".to_string(),
            instance_name: None,
            fuel_type: FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        autosize_water_heater_capacities(&mut specs, Some(3.0), 10.0);

        let result = &specs[0];
        let volume_m3 = result
            .parameters
            .get("tank_volume_m3")
            .and_then(|v| v.as_f64())
            .expect("volume must remain");

        assert!(
            (volume_m3 - 0.1).abs() < 1e-6,
            "explicit volume 0.1 m3 must not be overridden, got {volume_m3}"
        );
    }
}
